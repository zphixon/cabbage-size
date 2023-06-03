use chrono::{DateTime, Utc};
use rocket::{http::Status, response::status::Custom, serde::json::Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const USER_ID_URL: &'static str = "https://api.twitch.tv/helix/users";
pub const TOKEN_URL: &'static str = "https://id.twitch.tv/oauth2/token";

#[derive(Debug, Deserialize)]
pub struct Users {
    pub data: Vec<User>,
}

#[derive(Deserialize, PartialEq, Eq, Hash, Debug, Clone)]
pub struct User {
    id: String,
    display_name: String,
}

impl std::fmt::Display for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name)
    }
}

#[derive(Debug, Deserialize)]
pub struct Auth {
    pub access_token: String,
    pub expires_in: i64,
}

#[derive(Debug)]
pub struct ClientId {
    pub client_id: &'static str,
}

#[derive(Debug)]
pub struct LastChecked {
    pub time: DateTime<Utc>,
    pub limit: Option<i64>,
    pub streamer: User,
    pub size: i64,
}

#[derive(Serialize)]
pub struct Size {
    pub size: i64,
    pub is_message: bool,
    pub message: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct BoonLength {
    pub time: DateTime<Utc>,
    pub length: i64,
}

#[derive(Debug, Clone, Copy)]
pub enum BoonKind {
    Cursed,
    Blessed,
}

#[derive(Debug, Clone, Copy)]
pub struct Boon {
    pub kind: BoonKind,
    pub value: Option<i64>,
    pub duration: Option<BoonLength>,
}

#[derive(Debug, Default)]
pub struct ChannelStatus {
    pub boons: HashMap<User, Boon>,
    pub active_boon: Option<Boon>,
    pub bounds: Bounds,
}

#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub upper: i64,
    pub lower: i64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            upper: 100,
            lower: 1,
        }
    }
}

impl BoonLength {
    pub fn active(&self) -> bool {
        (Utc::now() - self.time).num_seconds() <= self.length
    }
}

pub type SizeResponse = Custom<Json<Size>>;

pub fn ok_response(size: i64) -> SizeResponse {
    Custom(
        Status::Ok,
        Json(Size {
            size,
            is_message: false,
            message: "",
        }),
    )
}

pub fn make_response(status: Status, message: &'static str) -> SizeResponse {
    Custom(
        status,
        Json(Size {
            size: status.code as i64,
            is_message: true,
            message,
        }),
    )
}
