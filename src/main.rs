use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use tokio::process::Command;
use tokio::io::AsyncWriteExt;
use mime_guess::from_path;

fn determine_mime_type(path: &PathBuf) -> &'static str {
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

fn extract_headers(output: &str) -> (Vec<(String, String)>, usize) {
    let mut headers = Vec::new();
    let mut body_start_index = 0;

    for (index, line) in output.lines().enumerate() {
        if line.is_empty() {
            body_start_index = index + 1;
            break;
        }

        if let Some((key, value)) = line.split_once(":") {
            headers.push((key.trim().to_string(), value.trim().to_string()));
        }
    }

    (headers, body_start_index)
}

async fn process_request(mut stream: TcpStream, root_dir: PathBuf) {
    let mut buffer = [0; 1024];
    stream.read(&mut buffer).unwrap();
    let request = String::from_utf8_lossy(&buffer[..]);

    let (method, path, query) = analyze_request(&request);
    let full_path = root_dir.join(&path[1..]);
    let response = 
    if path.starts_with("/..") || path.starts_with("/forbidden") {
        println!("{} 127.0.0.1 {} -> 403 (Forbidden)", method, path);
        b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n<html>403 Forbidden</html>".to_vec()
    } else if path.starts_with("/scripts/") {
        match method.as_str() {
            "GET" | "POST" => {
                if full_path.is_file() {
                    let mut cmd = Command::new(&full_path);

                    // Set environment variables from query parameters
                    if let Some(query) = query {
                        let query_pairs = query.split('&').map(|pair| {
                            let mut split = pair.split('=');
                            (
                                split.next().unwrap_or("").to_string(),
                                split.next().unwrap_or("").to_string(),
                            )
                        });

                        for (key, value) in query_pairs {
                            let env_var = format!("Query_{}", key);
                            cmd.env(env_var, value);
                        }
                    }

                    // Additional environment variables required by the script
                    cmd.env("Method", method.as_str());
                    cmd.env("Path", path.as_str());

                    let output = if method == "GET" {
                        cmd.stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .output()
                            .await
                            .expect("Failed to execute script")
                    } else {
                        let body = extract_request_body(&request);
                        let mut child = cmd.stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                            .expect("Failed to execute script");

                        if let Some(stdin) = child.stdin.as_mut() {
                            stdin.write_all(body.as_bytes()).await.expect("Failed to write to stdin");
                        }

                        child.wait_with_output().await.expect("Failed to read stdout")
                    };

                    if output.status.success() {
                        let output_str = String::from_utf8_lossy(&output.stdout);
                        let (headers, body_start_index) = extract_headers(&output_str);
                        let body = output_str.lines().skip(body_start_index).collect::<Vec<_>>().join("\n");
                        let content_type = headers.iter().find(|&&(ref k, _)| k == "Content-type")
                            .map(|&(_, ref v)| v.clone())
                            .unwrap_or_else(|| "text/plain".to_string());
                        let content_length = headers.iter().find(|&&(ref k, _)| k == "Content-length")
                            .map(|&(_, ref v)| v.clone())
                            .unwrap_or_else(|| body.len().to_string());

                        println!("{} 127.0.0.1 {} -> 200 (OK)",method, path);

                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            content_type, content_length, body
                        ).as_bytes().to_vec()
                    } else {
                        println!("{} 127.0.0.1 {} -> 500 (Internal Server Error)",method, path);
                        b"HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\n\r\n<html>500 Internal Server Error</html>".to_vec()
                    }
                } else {
                    println!("{} 127.0.0.1 {} -> 404 (Not Found)",method, path);
                    b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n<html>404 Not Found</html>".to_vec()
                }
            }
            _ => {
                println!("{} 127.0.0.1 {} -> 405 (Method Not Allowed)",method, path);
                b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n<html>405 Method Not Allowed</html>".to_vec()
            }
        }
    } else {
        match method.as_str() {
            "GET" => {
                if full_path.is_file() {
                    let contents = fs::read(&full_path).expect("Unable to read file");
                    let mime_type = determine_mime_type(&full_path);

                    println!("{} 127.0.0.1 {} -> 200 (OK)", method, path);
                    let mut response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        mime_type,
                        contents.len(),
                    ).as_bytes().to_vec();
                    response.extend_from_slice(&contents);
                    response
                } else {
                    println!("{} 127.0.0.1 {} -> 404 (Not Found)",method, path);
                    b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n<html>404 Not Found</html>".to_vec()
                }
            }
            _ => {
                println!("{} 127.0.0.1 {} -> 405 (Method Not Allowed)",method, path);
                b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n<html>405 Method Not Allowed</html>".to_vec()
            }
        }
    };

    stream.write_all(&response).unwrap();
    stream.flush().unwrap();
}

fn analyze_request(request: &str) -> (String, String, Option<String>) {
    let lines: Vec<&str> = request.lines().collect();
    if lines.is_empty() {
        return ("".to_string(), "".to_string(), None);
    }

    let mut parts = lines[0].split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let mut path = parts.next().unwrap_or("").to_string();
    let query = if let Some(index) = path.find('?') {
        let query = path.split_off(index + 1);
        path.pop(); // Remove the '?' from the end of path
        Some(query)
    } else {
        None
    };
    (method, path, query)
}

fn extract_request_body(request: &str) -> String {
    if let Some(index) = request.find("\r\n\r\n") {
        request[index + 4..].to_string()
    } else {
        "".to_string()
    }
}

#[tokio::main]
async fn run() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <port> <root_folder>", args[0]);
        return;
    }

    let port: u16 = args[1].parse().expect("Invalid port number");
    let root_dir = PathBuf::from(&args[2]);

    let listener = TcpListener::bind(("0.0.0.0", port)).expect("Failed to bind to address");

    println!("Root folder: {:?}", root_dir.canonicalize().unwrap());
    println!("Server listening on 0.0.0.0:{}", port);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let root_dir = root_dir.clone();
                tokio::spawn(async move {
                    process_request(stream, root_dir).await;
                });
            }
            Err(e) => {
                eprintln!("Connection failed: {}", e);
            }
        }
    }
}

fn main() {
    run();
}
