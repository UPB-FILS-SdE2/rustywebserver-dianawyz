[![Review Assignment Due Date](https://classroom.github.com/assets/deadline-readme-button-22041afd0340ce965d47ae6ef1cefeee28c7c493a6346c4f15d667ab976d596c.svg)](https://classroom.github.com/a/TXciPqtn)
# Rustwebserver

Detail the homework implementation.

## Functions:
Explaining the functions I used and what do they do.

### main
In the main function, it firstly parses the command-line arguments to get the port and root folder, sets up a TCP listener on the specified port and handles incoming connections using threads

### handle_client
The function processes incoming client requests. Firstly, it reads the incoming request, then parses the request line and headers. Determines the full path based on the root folder and requested path, then handles GET and POST requests appropriately and after, it sends the HTTP response back to the client

### handle_get_request
It processes 'GET' requests. Checks if the requested file exists, then if the path is a directory, returns a directory listing. It reads and returns the file content with the appropriate MIME type:
* Returns 403 Forbidden if the file cannot be read
* Returns 404 Not Found if the file does not exist

### handle_post_request
This function processes POST requests. Checks if the requested script exists in the '/scripts' directory, then executes the script with the provided headers and body. Lastly, returns the script's output:
* Returns 403 Forbidden if attempting to access files outside the scripts directory
* Returns 404 Not Found if the script does not exist

### http_response
This function constructs an HTTP response string. It includes:
    * Status code
    * Status text
    * Headers
    * Body

### generate_directory_listing
This function generates an HTML response. It lists the contents of a directory, including a link to the parent directory

### get_status_text
The function returns the status text corresponding to a given status code

