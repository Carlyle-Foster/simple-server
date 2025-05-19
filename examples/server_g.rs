use std::net::SocketAddr;
use std::time::Duration;

use mio::net::TcpStream;
use rustls::ServerConnection;
use simple_server::{server_G, HttpServer2};
use simple_server::websocket::WsServer;
use simple_server::helpers::{get_domain_certs, get_private_key, get_ssl_config};
// use simple_server::websocket::{Message, WebSocket, WebSocketError};

type Cl = server_G::Client<simple_server::websocket::Messenger, simple_server::websocket::WsParser, simple_server::websocket::Message, simple_server::websocket::WebSocketError>;

fn main() {
    let domain_cert = get_domain_certs("https_certificates/domain.cert.pem");
    let private_key = get_private_key("https_certificates/private.key.pem");

    let config = get_ssl_config(domain_cert, private_key);

    let mut server = WsServer::new(SocketAddr::from(([127, 0, 0, 1], 8782)), config);
    server.heartbeat = Some(Duration::from_millis(500));

    let mut dots = 0;
    println!("size of Client: {}", size_of::<Cl>());
    println!("    size of id: {}", size_of::<usize>());
    println!("    size of TlStream: {}", size_of::<simple_server::TLS::TLStream>());
    println!("        size of TcpStream: {}", size_of::<TcpStream>());
    println!("        size of ServerConnection: {}", size_of::<ServerConnection>());
    println!("    size of buffer: {}", size_of::<simple_server::Buffer>());
    println!("    size of messenger: {}", size_of::<simple_server::websocket::Messenger>());
    println!("    size of parser: {}", size_of::<simple_server::websocket::WsParser>());
    
    loop {
        for (id, request) in server.into_iter() {
            println!("id = {id}");
            println!("{request:#?}");
        };
        for _ in 0..dots {
            print!(".");
        }
        println!();
        dots += 1;
        dots = dots % 4;
    }
}

// fn serve_client_directory() -> impl FnMut(Request) -> Utf8PathBuf {
//     move |request: Request| -> Utf8PathBuf {
//         request.path
//     }
// }

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