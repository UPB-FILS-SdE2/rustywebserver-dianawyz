use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use mime_guess::from_path;
use tokio::runtime::Runtime;

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: rustwebserver PORT ROOT_FOLDER");
        return Ok(());
    }
    
    let port = &args[1];
    let root_folder = Arc::new(PathBuf::from(&args[2]));

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))?;
    println!("Root folder: {:?}", fs::canonicalize(&*root_folder).unwrap());
    println!("Server listening on 0.0.0.0:{}", port);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let root_folder = Arc::clone(&root_folder);
                thread::spawn(move || {
                    handle_client(stream, root_folder).unwrap_or_else(|error| eprintln!("{:?}", error));
                });
            },
            Err(e) => eprintln!("Connection failed: {}", e),
        }
    }

    Ok(())
}

fn handle_client(mut stream: TcpStream, root_folder: Arc<PathBuf>) -> io::Result<()> {
    let mut buffer = [0; 8192];
    stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer);

    let mut lines = request.lines();
    if let Some(request_line) = lines.next() {
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() == 3 {
            let method = parts[0];
            let path = parts[1];
            match method {
                "GET" => handle_get_request(&mut stream, &root_folder, path),
                "POST" => handle_post_request(&mut stream, &root_folder, path, &buffer),
                _ => respond_with_status(&mut stream, 405, "Method Not Allowed", None),
            }?;
        }
    }

    Ok(())
}

fn handle_get_request(stream: &mut TcpStream, root_folder: &Arc<PathBuf>, path: &str) -> io::Result<()> {
    let full_path = root_folder.join(&path[1..]);

    if !full_path.exists() {
        log_request("GET", path, 404, "Not Found");
        return respond_with_status(stream, 404, "Not Found", None);
    }

    if full_path.is_dir() {
        return generate_directory_listing(stream, &full_path);
    }

    match fs::read(&full_path) {
        Ok(contents) => {
            let mime_type = from_path(&full_path).first_or_octet_stream().to_string();
            log_request("GET", path, 200, "OK");
            respond_with_status(stream, 200, "OK", Some((mime_type, contents)))
        },
        Err(_) => {
            log_request("GET", path, 403, "Forbidden");
            respond_with_status(stream, 403, "Forbidden", None)
        }
    }
}

fn handle_post_request(stream: &mut TcpStream, root_folder: &Arc<PathBuf>, path: &str, buffer: &[u8]) -> io::Result<()> {
    let full_path = root_folder.join(&path[1..]);

    if !full_path.exists() || !full_path.is_file() {
        log_request("POST", path, 404, "Not Found");
        return respond_with_status(stream, 404, "Not Found", None);
    }

    if !full_path.starts_with("scripts") {
        log_request("POST", path, 403, "Forbidden");
        return respond_with_status(stream, 403, "Forbidden", None);
    }

    let body_start = buffer.windows(4).position(|window| window == b"\r\n\r\n").unwrap_or(buffer.len());
    let body = &buffer[body_start + 4..];

    let mut command = Command::new(&full_path);
    command.env("METHOD", "POST");
    command.env("PATH", path);
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let mut child = command.spawn().expect("failed to execute process");
    
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body)?;
    }

    let output = child.wait_with_output().expect("failed to wait on child");

    if output.status.success() {
        log_request("POST", path, 200, "OK");
        respond_with_status(stream, 200, "OK", Some(("text/plain".to_string(), output.stdout)))
    } else {
        log_request("POST", path, 500, "Internal Server Error");
        respond_with_status(stream, 500, "Internal Server Error", Some(("text/plain".to_string(), output.stderr)))
    }
}

fn generate_directory_listing(stream: &mut TcpStream, path: &Path) -> io::Result<()> {
    let mut response = String::from("<html><h1>Directory listing</h1><ul>");
    response.push_str(&format!("<li><a href=\"..\">..</a></li>"));

    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries {
            if let Ok(entry) = entry {
                let entry_path = entry.path();
                response.push_str(&format!(
                    "<li><a href=\"/{}\">{}</a></li>",
                    entry_path.display(),
                    entry_path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }

    response.push_str("</ul></html>");
    respond_with_status(stream, 200, "OK", Some(("text/html; charset=utf-8".to_string(), response.into_bytes())))
}

fn respond_with_status(stream: &mut TcpStream, status_code: u16, status_text: &str, body: Option<(String, Vec<u8>)>) -> io::Result<()> {
    let response = if let Some((content_type, body)) = body {
        format!(
            "HTTP/1.0 {} {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
            status_code, status_text, content_type
        ) + &String::from_utf8_lossy(&body)
    } else {
        format!(
            "HTTP/1.0 {} {}\r\nConnection: close\r\n\r\n",
            status_code, status_text
        )
    };
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn log_request(method: &str, path: &str, status_code: u16, status_text: &str) {
    println!("{} {} -> {} ({})", method, path, status_code, status_text);
}
