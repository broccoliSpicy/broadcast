# Broadcast Server

A single-threaded TCP broadcast server

## Features
- Broadcast messages to all connected clients, any previous messages before the client establish connection will also be sent to the client.
- Handle multiple clients concurrently
- This server does not handle persistency issues, no data is perserved to disk and all server state is lost when shut down.
- Out-of-memory issues may occur, and these are not handled by the server, which could lead to crashes or undefined behavior if resource limits are exceeded.

## Getting Started

### Running the Server

1. Clone the repository:

    ```sh
    git clone https://github.com/broccoliSpicy/broadcast
    cd broadcast
    ```

2. Run the server:

    ```sh
    cargo run
    ```

### Running Tests

To run the tests, run the following command:

```sh
cargo test