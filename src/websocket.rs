use core::{fmt, str};
use std::{collections::HashMap, error::Error, sync::mpsc::{Receiver, TryRecvError}, time::Duration};

use mio::Token;
use sha1::{Digest, Sha1};
use base64::prelude::*;

use crate::{serve_client, ClientManifest, Protocol, Vfs, TLS::TLStream};

pub fn compute_sec_websocket_accept(key: &str) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
    BASE64_STANDARD.encode(sha1.finalize())
}

pub struct WebSocket {
    incoming_message: Option<Message>,
    //TODO: change this
    receiver: Receiver<TLStream>,
    queue: mio::Events,
    poll: mio::Poll,
    clients: ClientManifest,
    buffer: Vec<u8>,
}

impl WebSocket {
    pub fn new(receiver: Receiver<TLStream>) -> Self {
        Self {
            incoming_message: None, 
            receiver, 
            queue: mio::Events::with_capacity(128),
            poll: mio::Poll::new().unwrap(),
            clients: ClientManifest::new(128),
            buffer: Vec::with_capacity(1024),
        }
    }
    fn welcome_new_clients(&mut self) {
        loop {
            match self.receiver.try_recv() {
                Ok(stream) => {
                    println!("added 1 client");
                    self.clients.insert(stream, Protocol::WEBSOCKET, self.poll.registry()).unwrap();
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => panic!(),
            }
        }
    }
    pub fn read_message(&mut self) -> Result<Message, WebSocketError> {
        use WebSocketError::*;

        self.welcome_new_clients();
        self.poll.poll(&mut self.queue, Some(Duration::ZERO)).unwrap();
        for event in &self.queue {
            match serve_client(&mut self.buffer, &mut Vfs(HashMap::new()), event, &mut self.clients) {
                Ok(0) => {},
                Ok(bytes_read) => return self.parse_message(bytes_read),
                Err(e) => {
                    println!("WEBSOCKET: dropped client {} on account of error {e}", event.token().0);
                    self.clients.remove(event.token());
                }
            };
        };
        Err(WOULD_BLOCK)
    }
    fn parse_message(&mut self, bytes_read: usize) -> Result<Message, WebSocketError> {
        use OPCODE::*;
        use WebSocketError::*;
        loop {
            let frame = Self::read_frame(self, &self.buffer[..bytes_read])?;
            match frame.opcode {
                Continuation => {
                    if let Some(message) = self.incoming_message.take() {
                        self.incoming_message = match message {
                            Message::Text(s, t) => Message::Text(s, frame.unmask_into_text(t)?),
                            Message::Binary(s, b) => Message::Binary(s, frame.unmask_into_binary(b)),
                        }.into();
                        if frame.fin { return Ok(self.incoming_message.take().unwrap()); }
                        else { continue }
                    }
                    else { return Err(BAD_CONTINUE) }
                }
                Text => {
                    if self.incoming_message.is_some() { return Err(CUTTING_IN) }
                    self.incoming_message = Some(Message::Text(Token(100), frame.unmask_into_text(String::with_capacity(256))?));
                    if frame.fin { return Ok(self.incoming_message.take().unwrap()); }
                    else { continue }
                },
                Binary => {
                    if self.incoming_message.is_some() { return Err(CUTTING_IN) }
                    self.incoming_message = Some(Message::Binary(Token(100), frame.unmask_into_binary(Vec::with_capacity(256))));
                    if frame.fin { return Ok(self.incoming_message.take().unwrap()); }
                    else { continue }
                }
                Close => { return Err(UNIMPLEMENTED) },
                Ping => { return Err(UNIMPLEMENTED) },
                Pong => { return Err(UNIMPLEMENTED) },
            }
        }
    }
    fn read_frame<'a>(&self, data: &'a [u8]) -> Result<Frame<'a>, WebSocketError> {
        use WebSocketError::*;

        if data.len() < (1 + 1 + 4) { return Err(TOO_SHORT) }
    
        let fin = ((data[0] >> 7) & 0b1) > 0;
        if ((data[0] >> 4) & 0b111) != 0 { return Err(UNRESERVED) }
        let opcode = OPCODE::parse(data[0] & 0b1111)?;
        if ((data[1] >> 7) & 0b1) == 0 { return Err(UNMASKED) }
        let mask_key_offset: usize;
        match (data[1] & 0b1111111) {
            l if l < 126 => {
                mask_key_offset = 2;
                l as u64
            },
            126 => {
                mask_key_offset = 4;
                if data.len() < 8 { return Err(TOO_SHORT) }
                u16::from_be_bytes(data[2..3].try_into().unwrap()) as u64
            },
            127 => {
                mask_key_offset = 10;
                if data.len() < 14 { return Err(TOO_SHORT) }
                u64::from_be_bytes(data[2..10].try_into().unwrap())
            }
            _ => unreachable!(),
        };
        let mask_key: [u8;4] = data[mask_key_offset..mask_key_offset+4].try_into().unwrap();
        let payload = &data[mask_key_offset+4..];
    
        let frame = Frame {
            fin,
            opcode,
            mask_key,
            payload,
        };
    
        println!("Frame: fin = {}, opcode = {:?}", frame.fin, frame.opcode);
    
        Ok(frame)
    }
    
}

#[derive(Debug)]
pub enum WebSocketError {
    TOO_SHORT,
    UNRESERVED,
    UNMASKED,
    BAD_OPCODE,
    BAD_CONTINUE,
    CUTTING_IN,
    EXPECTED_UTF8,
    CLOSED_BY_CLIENT,
    WOULD_BLOCK,
    UNIMPLEMENTED,
}

impl fmt::Display for WebSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for WebSocketError{}

#[derive(Debug)]
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
    pub mask_key: [u8;4],
    pub payload: &'buf [u8],
}

impl<'buf> Frame<'buf> {
    pub fn unmask_into_binary(&self, mut buffer: Vec<u8>) -> Vec<u8> {
        assert!(buffer.len() <= self.payload.len());
        for (index, byte)  in self.payload.iter().enumerate() {
            buffer.push(byte ^ self.mask_key[index % 4])
        }
        buffer
    }
    pub fn unmask_into_text(&self, buffer: String) -> Result<String, WebSocketError> {
        assert!(buffer.len() <= self.payload.len());

        use WebSocketError::*;

        let mut buffer = buffer.into_bytes();
        let initial_length = buffer.len();
        for (index, byte)  in self.payload.iter().enumerate() {
            buffer.push(byte ^ self.mask_key[index % 4])
        }
        let _ = str::from_utf8(&buffer[initial_length..]).map_err(|_e| EXPECTED_UTF8)?;
        return Ok( unsafe {String::from_utf8_unchecked(buffer)} )
    }
}

pub enum Message {
    Text(Token, String),
    Binary(Token, Vec<u8>),
}