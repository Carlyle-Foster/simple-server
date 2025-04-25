use std::net::SocketAddr;
use std::thread::sleep;
use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use simple_server::http::HttpServer;
use simple_server::http::{Method, Request};
use simple_server::helpers::{get_domain_certs, get_private_key};
// use simple_server::websocket::{Message, WebSocket, WebSocketError};

fn main() {
    let mut server = HttpServer::new();

    server.set_client_directory("client");
    server.add_service("/", Method::GET, serve_client_directory());
    server.set_homepage("index.html");
    server.set_404_page("client/missing.html");
    // server.set_websocket_handler(handle_websocket);

    let domain_cert = get_domain_certs("https_certificates/domain.cert.pem");
    let private_key = get_private_key("https_certificates/private.key.pem");

    server.serve(SocketAddr::from(([127, 0, 0, 1], 8783)), domain_cert, private_key);
}

fn serve_client_directory() -> impl FnMut(Request) -> Utf8PathBuf {
    move |request: Request| -> Utf8PathBuf {
        request.path
    }
}

// fn handle_websocket(mut socket: WebSocket) {
//     loop {
//         let start = Instant::now();
//         match socket.read_message() {
//             Ok(message) => {
//                 match message {
//                     (client, Message::Binary(bytes)) => {
//                         println!("WEBSOCKET_MESSAGE_BINARY: {{{:?}}}", bytes);
//                         socket.send_binary(&bytes, client).unwrap();
//                     }
//                     (client, Message::Text(text)) => {
//                         println!("WEBSOCKET_MESSAGE_TEXT: {text}");
//                         println!("WEBSOCKET_MESSAGE_LENGTH: {}", text.len());
//                         socket.send_text(&text, client).unwrap();
//                     }
//                 }
//             },
//             Err(WebSocketError::WOULD_BLOCK) => {},
//             Err(e) => panic!("{e}"),
//         }
//         let sleepy_time = Duration::from_millis(10).checked_sub(start.elapsed()).unwrap_or(Duration::from_millis(1));
//         sleep(sleepy_time);
//     }
// }

