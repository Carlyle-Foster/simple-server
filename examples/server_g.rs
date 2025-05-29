use std::net::SocketAddr;
use std::time::Duration;

use simple_server::server_G::Notification;
use simple_server::websocket::WsServer;
use simple_server::helpers::{get_domain_certs, get_private_key, get_ssl_config};

fn main() {
    let domain_cert = get_domain_certs("https_certificates/domain.cert.pem");
    let private_key = get_private_key("https_certificates/private.key.pem");

    let config = get_ssl_config(domain_cert, private_key);
    
    let mut server = WsServer::new(SocketAddr::from(([127, 0, 0, 1], 8782)), config);
    server.heartbeat = Some(Duration::from_millis(500));

    let mut dots = 0;
    loop {
        match server.serve() {
            Notification::SentMessage(id, message) => {
                println!("client {id} sent:");
                println!("{message:#?}");
                server.send_message(id, message);
            }
            Notification::Disconnected(id) => {
                println!("client {id} disconnected")
            }
            Notification::Heartbeat => {
                for _ in 0..dots {
                    print!(".");
                }
                println!();
                dots += 1;
                dots = dots % 4;
            }
        }
    }
}