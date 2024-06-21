use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use mime_guess::from_path;
use urlencoding::decode;

fn get_mime_type(path: &Path) -> &'static str {
    match from_path(path).first_or_octet_stream().essence_str() {
        "text/plain" => "text/plain; charset=utf-8",
        "text/html" => "text/html; charset=utf-8",
        "text/css" => "text/css; charset=utf-8",
        "application/javascript" => "text/javascript; charset=utf-8",
        "image/jpeg" => "image/jpeg",
        "image/png" => "image/png",
        "application/zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn main() {
    // Parse command-line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: rustwebserver PORT ROOT_FOLDER");
        return;
    }
    let port = &args[1];
    let root_folder = PathBuf::from(&args[2]);

    // Log root folder and server listening address
    println!("Root folder: {:?}", fs::canonicalize(&root_folder).unwrap());
    println!("Server listening on 0.0.0.0:{}", port);

    // Set up TCP listener
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).unwrap();
    let root_folder = Arc::new(root_folder);

    // Handle incoming connections
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let root_folder = Arc::clone(&root_folder);
                thread::spawn(move || {
                    handle_client(stream, &root_folder);
                });
            }
            Err(e) => eprintln!("Connection failed: {}", e),
        }
    }
}

fn handle_client(mut stream: TcpStream, root_folder: &Path) {
    let mut buffer = [0; 8192];
    match stream.read(&mut buffer) {
        Ok(size) => {
            if size == 0 {
                return;
            }
            let request = String::from_utf8_lossy(&buffer[..size]);
            let mut lines = request.lines();
            if let Some(request_line) = lines.next() {
                let mut parts = request_line.split_whitespace();
                if let (Some(method), Some(path), Some(_)) = (parts.next(), parts.next(), parts.next()) {
                    // Decode URL
                    let decoded_path = decode(path).unwrap_or_else(|_| path.to_string());

                    // Parse headers
                    let mut headers = Vec::new();
                    for line in lines {
                        if line.is_empty() {
                            break;
                        }
                        headers.push(line.to_string());
                    }

                    // Determine the full path and prevent directory traversal
                    let full_path = root_folder.join(&decoded_path[1..]).canonicalize();
                    match full_path {
                        Ok(full_path) => {
                            if !full_path.starts_with(root_folder) {
                                let response = http_response(403, "Forbidden", None, None);
                                let _ = stream.write_all(response.as_bytes());
                                return;
                            }

                            let response = match method {
                                "GET" => handle_get_request(&full_path, &headers),
                                "POST" => handle_post_request(&full_path, &headers, &buffer[size..]),
                                _ => http_response(405, "Method Not Allowed", None, None),
                            };

                            // Send response
                            let _ = stream.write_all(response.as_bytes());
                            stream.flush().unwrap();

                            // Log request
                            let status_code = response.split_whitespace().nth(1).unwrap_or("500");
                            let client_addr = stream.peer_addr().unwrap();
                            println!("{} {} -> {}", method, path, status_code);
                        }
                        Err(_) => {
                            let response = http_response(404, "Not Found", None, None);
                            let _ = stream.write_all(response.as_bytes());
                        }
                    }
                } else {
                    let response = http_response(400, "Bad Request", None, None);
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        }
        Err(e) => eprintln!("Failed to read from connection: {}", e),
    }
}

fn handle_get_request(full_path: &Path, headers: &[String]) -> String {
    if !full_path.exists() {
        return http_response(404, "Not Found", None, None);
    }
    if full_path.is_dir() {
        return generate_directory_listing(full_path);
    }

    match fs::read(full_path) {
        Ok(contents) => {
            let mime_type = get_mime_type(full_path);
            http_response(200, "OK", Some(mime_type), Some(&contents))
        }
        Err(_) => http_response(403, "Forbidden", None, None),
    }
}

fn handle_post_request(full_path: &Path, headers: &[String], body: &[u8]) -> String {
    if !full_path.exists() || !full_path.is_file() {
        return http_response(404, "Not Found", None, None);
    }

    if !full_path.starts_with("scripts") {
        return http_response(403, "Forbidden", None, None);
    }

    let mut command = Command::new(full_path);
    for header in headers {
        if let Some((key, value)) = header.split_once(':') {
            command.env(key.trim(), value.trim());
        }
    }

    command.env("Method", "POST");
    command.env("Path", full_path.to_str().unwrap());
    command.stdin(Stdio::piped());

    let mut child = command.spawn().unwrap();
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body).unwrap();
    }

    let output = child.wait_with_output().unwrap();
    if output.status.success() {
        http_response(200, "OK", None, Some(&output.stdout))
    } else {
        http_response(500, "Internal Server Error", None, Some(&output.stderr))
    }
}

fn http_response(status_code: u16, status_text: &str, content_type: Option<&str>, body: Option<&[u8]>) -> String {
    let mut response = format!("HTTP/1.0 {} {}\r\n", status_code, status_text);
    if let Some(content_type) = content_type {
        response.push_str(&format!("Content-Type: {}\r\n", content_type));
    }
    response.push_str("Connection: close\r\n\r\n");
    if let Some(body) = body {
        response.push_str(&String::from_utf8_lossy(body));
    }
    response
}

fn generate_directory_listing(path: &Path) -> String {
    let mut response = String::new();
    response.push_str("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n");
    response.push_str("<html><h1>Directory listing</h1><ul>");
    response.push_str(&format!("<li><a href=\"{}\">..</a></li>", path.parent().unwrap().display()));

    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let entry_path = entry.path();
        response.push_str(&format!(
            "<li><a href=\"/{}\">{}</a></li>",
            entry_path.display(),
            entry_path.file_name().unwrap().to_string_lossy()
        ));
    }

    response.push_str("</ul></html>");
    response
}
