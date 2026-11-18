use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    Layout { spine: u32, text: String, width: f32, height: f32, margin: f32 },
    Ping,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    Ready { spine: u32, hash: u64, ir: Vec<u8> },
    Pong,
    Error { message: String },
}

pub fn handle(bytes: &[u8]) -> Vec<u8> {
    let request = match postcard::from_bytes::<Request>(bytes) {
        Ok(request) => request,
        Err(_) => {
            return postcard::to_allocvec(&Response::Error { message: String::from("decode") })
                .unwrap_or_default()
        }
    };
    let response = match request {
        Request::Layout { spine, text, width, height, margin } => {
            let ir = crate::layout_text(&text, width, height, margin);
            let hash = ebook_ir::codec::hash_of(&ir);
            Response::Ready { spine, hash, ir }
        }
        Request::Ping => Response::Pong,
    };
    postcard::to_allocvec(&response).unwrap_or_default()
}
