/// Single Thread TCP broadcast server
/// Author: Jun Wang
/// Mar 8, 2025, in Boston, windy, cloudy, cold.
///
///
/// This is a single threaded TCP broadcast server, it acheives concurrency by using `tokio` asynchronous programming.
/// It avoids any usage of synchronization primitives by storing each client's message separately and each client has a dedicated
/// asynchronmous task sending messages to client. Each client's message queue is updated(added and deleted)
/// safely because these two operations are never `awaited` and there is no parallism in this program.
///
/// This server uses the client's port number as its `client_id`, this is fine in a localhost environment.
///
/// However, no measurement of persistency is taken and out-of-memory issues are not handled. No data is perserved to disk
/// and all server state is lost when shut down, crashes or undefined behavior may occur if resource limits are exceeded.
///
use std::collections::VecDeque;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::LocalSet;

const SERVER_ADDR: &str = "127.0.0.1";
const SERVER_PORT: u16 = 8888;

// `MESSAGES` are message queues for each of the client, since we are using client's port number
// as client identifier and there are u16::MAX port numbers in total, so `MESSAGES` has size u16::MAX.
static mut MESSAGES: [VecDeque<String>; u16::MAX as usize] = {
    const INIT: VecDeque<String> = VecDeque::new();
    [INIT; u16::MAX as usize]
};

// `CLIENTS` uses client_id as index to store clients' `OwnedWriteHalf`, None indicates this client is not connected.
// since we are using client's port number as client identifier and there are u16::MAX port numbers in total,
// so `CLIENTS` has size u16::MAX.
static mut CLIENTS: [Option<OwnedWriteHalf>; u16::MAX as usize] = {
    const INIT: Option<OwnedWriteHalf> = None;
    [INIT; u16::MAX as usize]
};

// "current_thread" sets all `tokio`` tasks run in the same thread as main.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    run_server().await;
}

async fn run_server() {
    let addr = format!("{}:{}", SERVER_ADDR, SERVER_PORT);

    let listener = TcpListener::bind(addr).await.unwrap();
    println!("listening on port {}", SERVER_PORT);
    let local_set = LocalSet::new();
    local_set
        .run_until(async {
            loop {
                let (socket, addr) = listener.accept().await.unwrap();
                println!("connected {} {}", addr.ip(), addr.port());

                // use client's port number as client_id, this is fine in a localhost environment.
                let client_id = addr.port() as usize;

                // for each client, spawn a asynchronous `handle_client` task within the same thread.
                tokio::task::spawn_local(async move {
                    handle_client(socket, client_id).await;
                });
            }
        })
        .await;
}

// `handle_client` handles client's connection, for each client connection, it first calls `send_message` to push the `LOGIN` message
// to the front of this client's message queue, then spawns an asynchronous task handling messages sending to this client,
// then it loops on:
//      1. await`s on this client's sending messages,
//      2. calling `broadcast` for any received messages
//      3. calling `send_message` to push `ACK:MESSAGE` message to the front of this client's message queue.
async fn handle_client(socket: TcpStream, client_id: usize) {
    let (reader, writer) = socket.into_split();
    let mut reader = BufReader::new(reader);

    unsafe {
        // SAFETY: no parallelism in this program and all `CLIENTS` array update operations are not `awaited`.
        CLIENTS[client_id] = Some(writer);
    }

    send_message(client_id, format!("LOGIN:{}\n", client_id));

    let process_messages = tokio::task::spawn_local(async move {
        // Within the same thread, spawn a asynchronmous message sending task for each client.
        process_message_queue(client_id).await;
    });

    let mut line = String::new();

    while reader.read_line(&mut line).await.unwrap() > 0 {
        let message = line.trim().to_string();
        let broadcast_message = format!("MESSAGE:{} {}\n", client_id, message);
        print!("{}", broadcast_message);

        broadcast(client_id, broadcast_message);

        send_message(client_id, "ACK:MESSAGE\n".to_string());

        line.clear();
    }

    println!("disconnected {}", client_id);
    unsafe {
        // SAFETY: no parallism in this program and all `CLIENTS` array update operations are not `awaited`.
        CLIENTS[client_id] = None;
    }
    // do not clear MESSAGES[client_id], so when this client logs in again, it can resume receiving messages.

    process_messages.abort();
}

// `broadcast`` appends messages to all the remaining clients' message queue.
fn broadcast(sender_id: usize, message: String) {
    unsafe {
        // SAFETY: no parallelism in the program and all `MESSAGES` update operation are not `awaited`.
        for (id, queue) in MESSAGES.iter_mut().enumerate() {
            if id != sender_id {
                queue.push_back(message.clone());
            }
        }
    }
}

// `send_message` deals with situations when server needs to send its own response message to the client,
// right now, this is the `LOGIN` message and the `ACK:MESSAGE`.
fn send_message(client_id: usize, message: String) {
    unsafe {
        // SAFETY: no parallelism in the program and all `MESSAGES` update operations are not `awaited`.
        let queue = &mut MESSAGES[client_id];
        queue.push_front(message);
    }
}

// each connected client has a dedicated asynchronous `process_message_queue` task, it takes out messages from
// client's message queue and send it to this client one by one, so this client's messages are never interleaved.
async fn process_message_queue(client_id: usize) {
    loop {
        let writer = unsafe {
            // SAFETY: no parallelism in the program and `CLIENTS` update operation is never `awaited`.
            CLIENTS[client_id].as_mut()
        };

        if writer.is_none() {
            break;
        }
        let writer = writer.unwrap();

        let message = unsafe {
            // SAFETY: no parallelism in the program and `MESSAGES` update operation is never `awaited`.
            MESSAGES[client_id].pop_front()
        };

        if let Some(msg) = message {
            if let Err(e) = writer.write_all(msg.as_bytes()).await {
                eprintln!("Failed to send message to client {}: {}", client_id, e);
                break;
            }
        } else {
            tokio::task::yield_now().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    };
    use tokio::process::Command;

    async fn start_client() -> (OwnedWriteHalf, BufReader<OwnedReadHalf>) {
        // Connect to the server
        let mut attempts = 0;
        let stream = loop {
            match TcpStream::connect(format!("{}:{}", SERVER_ADDR, SERVER_PORT)).await {
                Ok(stream) => break stream,
                Err(e) => {
                    attempts += 1;
                    if attempts > 5 {
                        panic!("Failed to connect to server after 5 attempts: {}", e);
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        };

        let (reader, writer) = stream.into_split();
        let reader = BufReader::new(reader);

        (writer, reader)
    }

    #[tokio::test]
    async fn test_client_connection() {
        // start server
        let mut server = Command::new("cargo")
            .arg("run")
            .spawn()
            .expect("Failed to start the server");

        let (_writer, mut reader) = start_client().await;
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("Failed to read line");
        assert!(line.starts_with("LOGIN:"));

        // kill server
        server.kill().await.expect("Failed to kill the server");
    }

    #[tokio::test]
    async fn test_client_message_broadcast() {
        // start server
        let mut server = Command::new("cargo")
            .arg("run")
            .spawn()
            .expect("Failed to start the server");

        let (mut alice, mut alice_reader) = start_client().await;
        let (mut bob, mut bob_reader) = start_client().await;

        let mut line = String::new();
        alice_reader
            .read_line(&mut line)
            .await
            .expect("Failed to read login message");
        line.clear();

        bob_reader
            .read_line(&mut line)
            .await
            .expect("Failed to read login message");
        line.clear();

        const ALICE_MESSAGE: &str = "REQUEST\n";
        alice
            .write_all(ALICE_MESSAGE.as_bytes())
            .await
            .expect("Failed to send message");

        // Ensure alice gets ACK
        alice_reader
            .read_line(&mut line)
            .await
            .expect("Failed to read ack message");
        assert_eq!(line.trim(), "ACK:MESSAGE");
        line.clear();

        // Ensure bob receives broadcast
        bob_reader
            .read_line(&mut line)
            .await
            .expect("Failed to read broadcast message");
        assert!(line.contains("MESSAGE:"));
        assert!(line.contains(ALICE_MESSAGE));
        line.clear();

        const BOB_MESSAGE: &str = "REPLY\n";
        bob.write_all(BOB_MESSAGE.as_bytes())
            .await
            .expect("Failed to send message");

        // Ensure Alice receives broadcast
        alice_reader
            .read_line(&mut line)
            .await
            .expect("Failed to read broadcast message");
        assert!(line.contains("MESSAGE:"));
        assert!(line.contains(BOB_MESSAGE));
        line.clear();

        let (_john, mut john_reader) = start_client().await;

        // Ensure John receives broadcast
        john_reader
            .read_line(&mut line)
            .await
            .expect("Failed to read broadcast message");
        line.clear(); // LOGIN message

        john_reader
            .read_line(&mut line)
            .await
            .expect("Failed to read broadcast message");

        assert!(line.contains("MESSAGE:"));
        assert!(line.contains(ALICE_MESSAGE));
        line.clear();

        john_reader
            .read_line(&mut line)
            .await
            .expect("Failed to read broadcast message");

        assert!(line.contains("MESSAGE:"));
        assert!(line.contains(BOB_MESSAGE));
        line.clear();

        // kill the server
        server.kill().await.expect("Failed to kill the server");
    }
}
