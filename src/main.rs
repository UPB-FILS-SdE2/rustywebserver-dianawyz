use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use std::process::Stdio;

#[tokio::main]
async fn main() -> io::Result<()> {
    // Parse command-line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <port> <root_folder>", args[0]);
        std::process::exit(1);
    }

    // Extract port number and root folder from command-line arguments
    let port = args[1].parse::<u16>().expect("Invalid port number");
    let root_folder = PathBuf::from(&args[2]);

    // Print startup information
    println!("Root folder: {}", root_folder.display());
    println!("Server listening on 0.0.0.0:{}", port);

    // Start TCP listener
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))?;
    loop {
        // Accept incoming connections
        let (stream, _) = listener.accept()?;
        let root_folder = root_folder.clone();

        // Handle each connection in a separate asynchronous task
        tokio::spawn(async move {
            if let Err(e) = connection(stream, root_folder).await {
                eprintln!("Error handling connection: {}", e);
            }
        });
    }
}

/// Asynchronously handle each incoming TCP connection.
async fn connection(mut stream: TcpStream, root_folder: PathBuf) -> io::Result<()> {
    // Read HTTP request
    let mut buffer = [0; 1024];
    stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..]).to_string();
    let lines: Vec<&str> = request.lines().collect();

    // Parse the HTTP request
    let (method, path, query, headers) = {
        // Split request lines
        let lines: Vec<&str> = request.lines().collect();
        if lines.is_empty() {
            ("".to_string(), "".to_string(), None, vec![])
        } else {
            // Split the request line into method, path, and HTTP version
            let mut parts = lines[0].split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let mut path = parts.next().unwrap_or("").to_string();
            let _http_version = parts.next().unwrap_or(""); // Not used

            // Check if the path contains a query string
            let query = if let Some(index) = path.find('?') {
                let query = path.split_off(index + 1);
                path.pop();
                Some(query)
            } else {
                None
            };

            // Parse headers
            let mut headers = vec![];
            for line in lines.iter().skip(1) {
                if let Some((key, value)) = parse_header_line(line) {
                    headers.push((key, value));
                }
            }

            (method, path, query, headers)
        }
    };

    // Delegate to the appropriate handler based on the HTTP method
    match method.as_str() {
        "GET" => get( &mut stream, &root_folder, &path, query, &headers).await, 
        "POST" => post(&mut stream, &root_folder, &path, &request).await,
        _ => {
            println!("{} 127.0.0.1 {} -> 405 (Method Not Allowed)", method, path);
            let response = b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n<html>405 Method Not Allowed</html>";
            stream.write_all(response)?;
            Ok(())
        }
    }
}

// Asynchronous function to handle GET requests
async fn get(
    stream: &mut TcpStream,
    root_folder: &Path,
    path: &str,
    query: Option<String>,
    headers: &[(String, String)], // Add headers parameter
) -> io::Result<()> {

    // Construct the full path to the requested file
    let full_path = root_folder.join(&path[1..]); // Remove the leading '/' from the path

    // Check if the requested path is forbidden
    if path.starts_with("/..") || path.starts_with("/forbidden") {
        println!("GET 127.0.0.1 {} -> 403 (Forbidden)", path);
        let response = b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n<html>403 Forbidden</html>";
        stream.write_all(response)?;
        return Ok(());
    }

    // Handle scripts in the /scripts/ directory
    if path.starts_with("/scripts/") {
        match execute_script(&full_path, &query, path, "GET", headers).await { // Pass headers
            Ok(response) => stream.write_all(&response)?,
            Err(_) => {
                println!("GET 127.0.0.1 {} -> 500 (Internal Server Error)", path);
                let response =
                    b"HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\n\r\n<html>500 Internal Server Error</html>";
                stream.write_all(response)?;
            }
        }
        return Ok(());
    }

    // Serve static files if the path is not forbidden and not in /scripts/
    if full_path.is_file() {
        // Read the file contents
        let contents = fs::read(&full_path)?;

        // Determine the content type of the file
        let content_type = content_type(&full_path);

        // Construct the HTTP response
        println!("GET 127.0.0.1 {} -> 200 (OK)", path);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            content_type,
            contents.len(),
        );
        
        // Write the response header
        stream.write_all(response.as_bytes())?;
        
        // Write the file contents
        stream.write_all(&contents)?;
    } else {
        // File not found
        println!("GET 127.0.0.1 {} -> 404 (Not Found)", path);
        let response = b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n<html>404 Not Found</html>";
        stream.write_all(response)?;
    }

    Ok(())
}

// Function to execute scripts located in /scripts/ directory
async fn execute_script(
    script_path: &Path,
    query: &Option<String>,
    path: &str,
    method: &str,
    headers: &[(String, String)], // Add headers parameter
) -> io::Result<Vec<u8>> {
    if script_path.is_file() {
        let mut cmd = Command::new(&script_path);

        // Set environment variables from query parameters
        if let Some(query_string) = query {
            let query_pairs = query_string.split('&').map(|pair| {
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

        // Set environment variables from headers
        for (key, value) in headers {
            cmd.env(key, value);
        }

        cmd.env("Method", method);
        cmd.env("Path", path);

        let output = if method == "GET" {
            cmd.stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .expect("Failed to execute script")
        } else {
            unimplemented!("Handle non-GET method body handling here");
        };

        if output.status.success() {
            let output_str = String::from_utf8_lossy(&output.stdout);
            let (headers, body_start_index) = parse_headers(&output_str);
            let body = output_str.lines().skip(body_start_index).collect::<Vec<_>>().join("\n");
            let content_type = headers.iter().find(|&&(ref k, _)| k == "Content-type")
                .map(|&(_, ref v)| v.clone())
                .unwrap_or_else(|| "text/plain".to_string());
            let content_length = headers.iter().find(|&&(ref k, _)| k == "Content-length")
                .map(|&(_, ref v)| v.clone())
                .unwrap_or_else(|| body.len().to_string());

            println!("{} 127.0.0.1 {} -> 200 (OK)", method, path);

            Ok(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                content_type, content_length, body
            ).as_bytes().to_vec())
        } else {
            println!("{} 127.0.0.1 {} -> 500 (Internal Server Error)", method, path);
            Ok(b"HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\n\r\n<html>500 Internal Server Error</html>".to_vec())
        }
    } else {
        println!("{} 127.0.0.1 {} -> 404 (Not Found)", method, path);
        Ok(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n<html>404 Not Found</html>".to_vec())
    }
}


// Determine the content type based on file extension
fn content_type(file_path: &Path) -> &'static str {
    match file_path.extension().and_then(|ext| ext.to_str()) {
        Some("txt") => "text/plain; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("jpg") => "image/jpeg",
        Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

// Asynchronous function to handle POST requests
async fn post(
    stream: &mut TcpStream,
    root_folder: &PathBuf,
    path: &str,
    request: &str,
) -> io::Result<()> {
    let full_path = root_folder.join(&path[1..]);

    if full_path.is_file() {
        let mut cmd = Command::new(&full_path);

        // Extract request body to pass as input to script
        let body = extract_request_body(request);

        // Extract query string and set as environment variables
        if let Some(query) = extract_query_string(request) {
            let query_pairs = query.split('&').map(|pair| {
                let mut split = pair.split('=');
                (
                    format!("Query_{}", split.next().unwrap_or("")),
                    split.next().unwrap_or("").to_string(),
                )
            });
            for (key, value) in query_pairs {
                cmd.env(key, value);
            }
        }

        // Additional environment variables required by the script
        cmd.env("Method", "POST");
        cmd.env("Path", path);

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to execute script");

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(body.as_bytes()).await?;
        }

        let output = child
            .wait_with_output()
            .await
            .expect("Failed to read stdout");

        if output.status.success() {
            // Parse the output and headers from the script
            let output_str = String::from_utf8_lossy(&output.stdout);
            let (headers, body_start_index) = parse_headers(&output_str);

            // Find the start of the actual body content
            let body_content = output_str.lines().skip(body_start_index).collect::<Vec<_>>().join("\n");

            // Trim any trailing null terminators from the body content
            let trimmed_body = body_content.trim_end_matches(char::from(0));

            let content_type = headers
                .iter()
                .find(|&&(ref k, _)| k.to_lowercase() == "content-type")
                .map(|&(_, ref v)| v.clone())
                .unwrap_or_else(|| "text/plain".to_string());

            let content_length = trimmed_body.len(); // Calculate the trimmed body length

            println!("POST 127.0.0.1 {} -> 200 (OK)", path);

            // Construct the HTTP response
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                content_type, content_length, trimmed_body
            );

            // Write the response to the stream
            stream.write_all(response.as_bytes())?;
        } else {
            println!("POST 127.0.0.1 {} -> 500 (Internal Server Error)", path);
            let response = b"HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\n\r\n<html>500 Internal Server Error</html>";
            stream.write_all(response)?;
        }
    } else {
        println!("POST 127.0.0.1 {} -> 404 (Not Found)", path);
        let response = b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n<html>404 Not Found</html>";
        stream.write_all(response)?;
    }

    Ok(())
}

// Function to extract request body from the HTTP request
fn extract_request_body(request: &str) -> String {

    // Find the start of the body after headers
    if let Some(start_index) = request.find("\r\n\r\n") {
        let body_start = start_index + 4; // Skip "\r\n\r\n"
        request[body_start..].to_string()
    } else {
        String::new()
    }
}

// Function to extract query string from the HTTP request
fn extract_query_string(request: &str) -> Option<&str> {
    
    // Find the start of the request line
    if let Some(start_index) = request.find("\r\n") {
        let request_line = &request[..start_index];

        // Find the start of the query string (after the method and path)
        if let Some(path_index) = request_line.find(' ') {
            if let Some(query_start) = request_line[path_index..].find('?') {
                let query_start = path_index + query_start + 1; // Skip '?'
                if let Some(query_end) = request_line[query_start..].find(' ') {
                    return Some(&request_line[query_start..query_start + query_end]);
                }
            }
        }
    }

    None
}

// Function to parse headers from the script output
fn parse_headers(response: &str) -> (Vec<(String, String)>, usize) {
    let mut headers = Vec::new();
    let mut body_start_index = 0;

    // Split the response into lines
    let lines: Vec<&str> = response.lines().collect();

    // Iterate over lines to parse headers
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            // Empty line indicates end of headers, body starts after this
            body_start_index = index + 1;
            break;
        }

        // Split each line into key-value pairs
        if let Some(separator_index) = line.find(':') {
            let key = line[..separator_index].trim().to_string();
            let value = line[separator_index + 1..].trim().to_string();
            headers.push((key, value));
        }
    }

    (headers, body_start_index)
}

// Function to parse a single header line into key-value pair
fn parse_header_line(line: &str) -> Option<(String, String)> {
    if let Some(separator_index) = line.find(':') {
        let key = line[..separator_index].trim().to_string();
        let value = line[separator_index + 1..].trim().to_string();
        Some((key, value))
    } else {
        None
    }
}