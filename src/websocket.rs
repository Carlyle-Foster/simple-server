use core::{fmt, str};
use std::{collections::HashMap, error::Error, io::{self, ErrorKind, Write}, mem, sync::Arc, time::Duration};

use mio::{net::{TcpListener, TcpStream}, Interest, Token};
use rustls::ServerConfig;
use sha1::{Digest, Sha1};
use base64::prelude::*;

use crate::{helpers::{throw_reader_at_writer, Parser, SendTo}, http::Request, server_G::Server_G, smithy::HttpSmithText, Client, Protocol, SERVER, TLS::TLStream, TLS2::StreamId};

pub type WsServer = Server_G<Messenger, WsParser, Message, WebSocketError>;

impl WsServer {
    pub fn send_message(&mut self, id: StreamId, message: impl Into<Message>) {
        let m: Message = message.into();
        self.send_to_client(id, m);
    }
}

pub fn compute_sec_websocket_accept(key: &str) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
    BASE64_STANDARD.encode(sha1.finalize())
}

pub struct WebSocket {
    incoming_message: Option<Message>,
    //TODO: change this
    // receiver: Receiver<TLStream>,
    listener: TcpListener,
    config: Arc<ServerConfig>,
    queue: mio::Events,
    poll: mio::Poll,
    clients: HashMap<StreamId, Client>,
    events_processed: usize,
}

impl WebSocket {
    pub fn new(listener: TcpListener, config: ServerConfig) -> Self {
        Self {
            incoming_message: None, 
            listener,
            config: Arc::new(config),
            queue: mio::Events::with_capacity(128),
            poll: mio::Poll::new().unwrap(),
            clients: HashMap::with_capacity(128),
            events_processed: 0,
        }
    }
    pub fn read_message(&mut self) -> Result<(Token, Message), WebSocketError> {
        use WebSocketError::*;

        // self.welcome_new_clients();
        loop {
            match self.poll.poll(&mut self.queue, Some(Duration::from_millis(400))) {
                Ok(_) => {},
                Err(ref e) if e.kind() == ErrorKind::Interrupted => {},
                Err(e) => panic!("{e}"),
            }
            while let Some(event) =  self.queue.iter().nth(self.events_processed) {
                let token = event.token();
                let id = token.0 as StreamId;
                self.events_processed += 1;
                match id {
                    SERVER => {
                        match self.listener.accept() {
                            Ok((client, _)) => {
                                self.register(client).unwrap();
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                            Err(e) => {
                                println!("TLServer: connection refused due to error accepting: {:?}", e);
                            },
                        }
                    }
                    _client => {
                        let client = self.clients.get_mut(&id).unwrap();
                        let stream = &mut client.stream;
                        if stream.tls.is_handshaking() {
                            match client.stream.handshake() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::Interrupted => {},
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when handshaking: {e}");
                                    self.drop_client(id);
                                },
                            };
                            continue
                        }
                        if event.is_writable() {
                            match stream.flush() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::ConnectionAborted => {
                                    self.drop_client(id);
                                    break
                                },
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when flushing: {e}");
                                    self.drop_client(id);
                                    break
                                },
                            } 
                            match client.delivery.send_all(stream) {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) => {
                                    println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                                    self.drop_client(id);
                                    break
                                }
                            };
                        }
                        if event.is_readable() {
                            match throw_reader_at_writer(stream, &mut client.buf) {
                                Ok(()) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) => {
                                    println!("HTTP_SERVER: dropped client on account of error when reading: {e}");
                                    self.drop_client(id);
                                    continue
                                }
                            };
                            if !client.buf.has_read() { continue }
                            match parse_message(&mut self.incoming_message, client.buf.the_story_so_far()) {
                                Ok((msg, _rest)) => return Ok((token, msg)),
                                Err(WOULD_BLOCK) => continue,
                                Err(e) => {
                                    println!("HTTP_SERVER: dropped client on account of error when parsing message: {e}");
                                }
                            }
                        }
                    }
                }
            };
            self.events_processed = 0;
        }
    }
    fn register(&mut self, client: TcpStream) -> io::Result<StreamId> {
        let registry = self.poll.registry();
        let mut stream = TLStream::new(client, self.config.clone());
        let mut id = fastrand::usize(..);
        while self.clients.contains_key(&id) {
            id = fastrand::usize(..);
        }
        let token = Token(id as usize);
        let interests = Interest::READABLE | Interest::WRITABLE;
        registry.register(&mut stream, token, interests)?;

        let client = Client::new(id, Protocol::HTTP, stream);
        self.clients.insert(id, client);

        println!("WEBSOCKET: registered client {id}");

        return Ok(id)
    }
    //TODO: make multiple versions with different levels of abruptness
    pub fn drop_client(&mut self, id: StreamId) {
        let client = self.clients.get_mut(&id).unwrap();
        
        let registry = self.poll.registry();
        registry.deregister(&mut client.stream).unwrap();

        self.clients.remove(&id).unwrap();
    }
    pub fn send_binary(&mut self, message: &[u8], client: Token) -> Result<(), WebSocketError> {
        self.write_message(Message::Binary(message.to_owned()), client)
    }
    pub fn send_text(&mut self, message: &str, client: Token) -> Result<(), WebSocketError> {
        self.write_message(Message::Text(message.to_owned()), client)
    }
    fn write_message(&mut self, msg: Message, client: Token) -> Result<(), WebSocketError> {
        let f: Frame = (&msg).into();
        let len = f.payload.len();
        let mut buf = Vec::<u8>::with_capacity(len + 2 + 8);
        buf.push((1 << 7) | (f.opcode as u8));
        const limit1: usize = 126;
        const limit2: usize = 1 << 16;
        match len {
            0..limit1 => buf.push(len as u8),
            limit1..limit2 => {
                buf.push(limit1 as u8);
                buf.append(&mut (len as u16).to_be_bytes().to_vec());
            }
            limit2.. => {
                buf.push(127);
                buf.append(&mut (len as u64).to_be_bytes().to_vec());
            }
        }
        buf.append(&mut f.payload.to_owned());
        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct WsParser {
    incoming_message: Option<Message>,
    skip: usize,
    handshaker: HttpSmithText,
    is_handshaking: bool,
}

impl Parser<Message, WebSocketError> for WsParser {
    fn parse<'b>(&mut self, buf: &'b [u8]) -> Result<(Option<Message>, &'b [u8]), WebSocketError> {
        if self.is_handshaking {
            match self.handshaker.parse(buf) {
                Ok((Some(handshake), rest)) => {
                    verify_websocket_handshake(&handshake)?;
                    self.is_handshaking = true;
                    self.skip = buf.len() - rest.len();
                    Ok((None, rest))
                }
                Ok((None, rest)) => Ok((None, rest)),
                Err(_) => Err(WebSocketError::WET_HANDSHAKE),
            }
        }
        else {
            match parse_message(&mut self.incoming_message, &buf[self.skip..]) {
                Ok((msg, rest)) => {
                    self.skip = 0;
                    Ok((Some(msg), rest))
                },
                Err(WebSocketError::WOULD_BLOCK) => Ok((None, buf)),
                Err(e) => Err(e),
            }
        }
    }
}

fn verify_websocket_handshake(handshake: &Request) -> Result<(), WebSocketError> {

    Ok(())
}

fn parse_message<'b>(incoming_message: &mut Option<Message>, mut buf: &'b [u8]) -> Result<(Message, &'b [u8]), WebSocketError> {
    use OPCODE::*;
    use WebSocketError::*;
    loop {
        let (frame, rest) = read_frame(buf)?;
        buf = rest;
        match frame.opcode {
            Continuation => {
                if let Some(ref mut message) = incoming_message {
                    *incoming_message = match message {
                        Message::Text(t) => Message::Text(frame.unmask_into_text(mem::take(t))?),
                        Message::Binary(b) => Message::Binary(frame.unmask_into_binary(mem::take(b))),
                    }.into();
                    if frame.fin { 
                        return Ok((incoming_message.take().unwrap(), rest));
                    }
                    continue
                }
                else { return Err(BAD_CONTINUE) }
            }
            Text => {
                if incoming_message.is_some() { return Err(CUTTING_IN) }
                *incoming_message = Some(Message::Text(frame.unmask_into_text(String::with_capacity(256))?));
                if frame.fin { 
                    return Ok((incoming_message.take().unwrap(), rest)); 
                }
                else { continue }
            },
            Binary => {
                if incoming_message.is_some() { return Err(CUTTING_IN) }
                *incoming_message = Some(Message::Binary(frame.unmask_into_binary(Vec::with_capacity(256))));
                if frame.fin { 
                    return Ok((incoming_message.take().unwrap(), rest));
                }
                else { continue }
            }
            Close => { return Err(UNIMPLEMENTED) },
            Ping => { return Err(UNIMPLEMENTED) },
            Pong => { return Err(UNIMPLEMENTED) },
        }
    }
}
fn read_frame(data: &[u8]) -> Result<(Frame, &[u8]), WebSocketError> {
    use WebSocketError::*;

    let mut mask_offset = 1 + 1;
    const mask_len: usize = 4;
    if data.len() < (mask_offset + mask_len) { return Err(WOULD_BLOCK) }

    let fin = data[0] & 0b1000_0000 > 0;
    if (data[0] & 0b0111_0000) != 0 { return Err(UNRESERVED) }
    let opcode = OPCODE::parse(data[0] & 0b_1111)?;
    if (data[1] & 0b1000_0000) == 0 { return Err(UNMASKED) }
    let payload_len = match (data[1] & 0b0111_1111) {
        l if l < 126 => l as u64,
        126 => {
            mask_offset += 2;
            if data.len() < mask_offset + mask_len { return Err(WOULD_BLOCK) }
            u16::from_be_bytes(data[2..4].try_into().unwrap()) as u64
        },
        127 => {
            mask_offset += 8;
            if data.len() < mask_offset + mask_len { return Err(WOULD_BLOCK) }
            u64::from_be_bytes(data[2..10].try_into().unwrap())
        }
        _ => unreachable!(),
    };
    let mask_key: [u8;4] = data[mask_offset..][..mask_len].try_into().unwrap();
    let payload = &data[mask_offset + mask_len..];
    println!("PAYLOAD_LENGTH = {payload_len}, ACTUAL_LENGTH = {}", payload.len());
    let (payload, rest) = payload.split_at_checked(payload_len as usize).ok_or(WOULD_BLOCK)?;

    let frame = Frame {
        fin,
        opcode,
        mask_key: Some(mask_key),
        payload,
    };

    println!("Frame: fin = {}, opcode = {:?}", frame.fin, frame.opcode);

    Ok((frame, rest))
}

#[derive(Debug)]
#[derive(PartialEq)]
pub enum WebSocketError {
    UNRESERVED,
    UNMASKED,
    BAD_OPCODE,
    BAD_CONTINUE,
    CUTTING_IN,
    NOT_VALID_UTF8,
    CLOSED_BY_CLIENT,
    WOULD_BLOCK,
    WET_HANDSHAKE,
    UNIMPLEMENTED,
}

impl fmt::Display for WebSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for WebSocketError{}

#[derive(Debug, Clone, Copy)]
enum OPCODE {
    Continuation = 0b0,
    Text    = 0b0001,
    Binary  = 0b0010,
    Close   = 0b1000,
    Ping    = 0b1001,
    Pong    = 0b1010,
}

impl OPCODE {
    fn parse(value: u8) -> Result<Self, WebSocketError> {
        use OPCODE::*;
        use WebSocketError::*;

        let code = match value {
            0b0000 => Continuation,
            0b0001 => Text,
            0b0010 => Binary,
            0b1000 => Close,
            0b1001 => Ping,
            0b1010 => Pong,
            _ => return Err(BAD_OPCODE),
        };
        Ok(code)
    }
}

struct Frame<'buf> {
    pub fin: bool,
    pub opcode: OPCODE,
    pub mask_key: Option<[u8;4]>,
    pub payload: &'buf [u8],
}

impl Frame<'_> {
    pub fn unmask_into_binary(&self, mut buffer: Vec<u8>) -> Vec<u8> {
        assert!(buffer.len() <= self.payload.len());

        if let Some(mask_key) = self.mask_key {
            for (index, byte)  in self.payload.iter().enumerate() {
                buffer.push(byte ^ mask_key[index % 4])
            }
        } else {
            buffer.append(&mut self.payload.to_owned());
        }
        buffer
    }
    pub fn unmask_into_text(&self, buffer: String) -> Result<String, WebSocketError> {
        use WebSocketError::*;
        assert!(buffer.len() <= self.payload.len());

        let mut buffer = buffer.into_bytes();
        let initial_length = buffer.len();
        if let Some(mask_key) = self.mask_key {
            for (index, byte)  in self.payload.iter().enumerate() {
                buffer.push(byte ^ mask_key[index % 4])
            }
        } else {
            buffer.append(&mut self.payload.to_owned());
        }
        let _ = str::from_utf8(&buffer[initial_length..]).map_err(|_e| NOT_VALID_UTF8)?;
        return Ok( unsafe {String::from_utf8_unchecked(buffer)} )
    }    
}

#[derive(Debug, Clone)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

impl Default for Message {
    fn default() -> Self {
        Self::Binary(Vec::default())
    }
}

impl From<Vec<u8>> for Message {
    fn from(value: Vec<u8>) -> Self {
        Self::Binary(value)
    }
}
impl From<String> for Message {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl<'a> From<&'a Message> for Frame<'a> {
    fn from(msg: &'a Message) -> Self {
        match msg {
            Message::Binary(contents) => Frame {
                fin: true,
                opcode: OPCODE::Binary,
                mask_key: None,
                payload: contents,
            },
            Message::Text(contents) => Frame { 
                fin: true, 
                opcode: OPCODE::Text, 
                mask_key: None, 
                payload: contents.as_bytes(),
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Messenger {
    msg: Message,
    writ: usize,
}

impl From<Message> for Messenger {
    fn from(value: Message) -> Self {
        Self { msg: value, writ: 0 }
    }
}

impl SendTo for Messenger {
    fn send_to(&mut self, wr: &mut impl Write) -> io::Result<usize> {
        let f: Frame = (&self.msg).into();

        const limit1: usize = 126;
        const limit2: usize = 1 << 16;

        let len = f.payload.len();
        let header_len = match len {
            0..limit1 => 2,
            limit1..limit2 => 2 + 2,
            limit2.. => 2 + 8,
        };
        let bytes = if self.writ < header_len {
            let mut header = [0; 2 + 8];
            header[0] = (1 << 7) | (f.opcode as u8);
            header[1] = match header_len {
                2 => len as u8,
                4 => {
                    header[2..][..2].copy_from_slice(&(len as u16).to_be_bytes());
                    126
                },
                10 => {
                    header[2..][..8].copy_from_slice(&(len as u64).to_be_bytes());
                    127
                },
                _ => unreachable!()
            };
            wr.write(&header[self.writ..header_len])?
        }
        else {
            wr.write(&f.payload[self.writ - header_len..])?
        };

        self.writ += bytes;
        Ok(bytes)
    }
}