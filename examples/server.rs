use std::net::SocketAddr;

use camino::Utf8PathBuf;
use simple_server::Server;
use simple_server::http::{Method, Request};
use simple_server::helpers::{get_domain_certs, get_private_key, get_ssl_config};
// use simple_server::websocket::{Message, WebSocket, WebSocketError};

fn main() {
    let domain_cert = get_domain_certs("https_certificates/domain.cert.pem");
    let private_key = get_private_key("https_certificates/private.key.pem");

    let config = get_ssl_config(domain_cert, private_key);

    let mut server = Server::new(SocketAddr::from(([127, 0, 0, 1], 8783)), config);

    server.http.set_client_directory("fake_server/src");
    server.http.add_service("/", Method::GET, serve_client_directory());
    server.http.set_homepage("index.html");
    server.http.set_404_page("missing.html");
    // server.set_websocket_handler(handle_websocket);

    server.serve();

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

