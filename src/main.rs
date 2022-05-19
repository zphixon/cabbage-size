// vim: set nu rnu sw=4 et ai si

#[macro_use]
extern crate rocket;

use chrono::Utc;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use rand::Rng;
use rocket::{
    http::Status, response::status::Custom as Response, serde::json::Json, State,
};
use serde::{Deserialize, Serialize};

const USER_ID_URL: &'static str = "https://api.twitch.tv/helix/users";
const TOKEN_URL: &'static str = "https://id.twitch.tv/oauth2/token";

static VIEWERS: Lazy<DashMap<User, Vec<LastChecked>>> = Lazy::new(|| DashMap::new());
static STREAMERS: Lazy<DashMap<User, Bounds>> = Lazy::new(|| DashMap::new());
static USER_CACHE: Lazy<DashMap<String, User>> = Lazy::new(|| DashMap::new());

#[derive(Debug, Deserialize)]
struct Users {
    data: Vec<User>,
}

#[derive(Deserialize, PartialEq, Eq, Hash, Debug, Clone)]
struct User {
    id: String,
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct Auth {
    access_token: String,
    #[allow(dead_code)]
    expires_in: i64,
}

#[derive(Debug)]
struct ClientId {
    client_id: &'static str,
}

#[derive(Debug)]
struct LastChecked {
    time: chrono::DateTime<Utc>,
    limit: Option<i64>,
    streamer: User,
    size: i64,
}

#[derive(Serialize)]
struct SizeResponse {
    size: i64,
    is_message: bool,
    message: &'static str,
}

#[derive(Debug)]
struct Bounds {
    upper: i64,
    lower: i64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            upper: 100,
            lower: 1,
        }
    }
}

#[get("/cs")]
fn legacy() -> String {
    let rand: i64 = rand::thread_rng().gen_range(1..=100);
    format!("{rand}")
}

async fn get_user(
    auth: &State<Auth>,
    client_id: &State<ClientId>,
    user: &str,
) -> Result<User, Response<Json<SizeResponse>>> {
    if let Some(user_data) = USER_CACHE.get(user) {
        return Ok(user_data.clone());
    }

    let client = reqwest::Client::new();
    let params = [("login", user)];

    let user_response = client
        .get(USER_ID_URL)
        .query(&params)
        .bearer_auth(&auth.access_token)
        .header("Client-Id", client_id.client_id)
        .send()
        .await;

    let user_response: reqwest::Response = match user_response {
        Ok(user_response) => user_response,
        Err(_) => {
            return Err(Response(
                Status::InternalServerError,
                Json(SizeResponse {
                    size: 500,
                    is_message: true,
                    message: "twitch api request failed",
                }),
            ))
        }
    };

    let users = user_response.json::<Users>().await;
    let mut users = match users {
        Ok(users) => users,
        Err(e) => {
            dbg!(e);
            return Err(Response(
                Status::InternalServerError,
                Json(SizeResponse {
                    size: 500,
                    is_message: true,
                    message: "twitch json response nonsensical",
                }),
            ));
        }
    };

    if users.data.is_empty() {
        Err(Response(
            Status::InternalServerError,
            Json(SizeResponse {
                size: 500,
                is_message: true,
                message: "twitch returned no users",
            }),
        ))
    } else {
        let user_data = users.data.pop().unwrap();
        USER_CACHE.insert(user.into(), user_data.clone());
        Ok(user_data)
    }
}

fn make_size(bounds: &mut Bounds) -> i64 {
    let Bounds { upper, lower } = bounds;
    let rand: i64 = rand::thread_rng().gen_range(*lower..=*upper);
    if rand == *lower {
        *lower -= 1;
    } else if rand == *upper {
        *upper += 1;
    }
    rand
}

#[get("/size?<viewer>&<streamer>&<time_limit>")]
async fn size(
    viewer: &str,
    streamer: &str,
    time_limit: Option<i64>,
    auth: &State<Auth>,
    client_id: &State<ClientId>,
) -> Response<Json<SizeResponse>> {
    let viewer = match get_user(auth, client_id, viewer).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };

    let streamer = match get_user(auth, client_id, streamer).await {
        Ok(streamer) => streamer,
        Err(response) => return response,
    };

    let lc: &mut LastChecked;
    let mut last_checked_streams = VIEWERS.entry(viewer).or_default();
    if let Some(last_checked) = last_checked_streams
        .iter_mut()
        .find(|lc| lc.streamer == streamer)
    {
        // we have checked our size in this streamer's chat before
        if let Some(limit) = time_limit {
            // there is a time limit
            if (Utc::now() - last_checked.time).num_seconds() <= limit {
                // the time limit has not elapsed - do not update the size, just return it
                return Response(
                    Status::Ok,
                    Json(SizeResponse {
                        size: last_checked.size,
                        is_message: false,
                        message: "",
                    }),
                );
            } else {
                // we are past the time limit - get a new size
            }
        } else {
            // there is no time limit - get a new size
        }

        last_checked.time = Utc::now();
        last_checked.limit = time_limit;
        lc = last_checked;
    } else {
        // we have not checked our size in this streamer's chat before - get a new size
        last_checked_streams.push(LastChecked {
            streamer: streamer.clone(),
            limit: time_limit,
            time: Utc::now(),
            size: 0,
        });
        lc = last_checked_streams.last_mut().unwrap();
    }

    let mut bounds = STREAMERS.entry(streamer).or_default();
    let size = make_size(&mut bounds);
    lc.size = size;

    Response(
        Status::Ok,
        Json(SizeResponse {
            size,
            is_message: false,
            message: "",
        }),
    )
}

#[rocket::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client_id = include_str!("../client_id").trim();
    let client_secret = include_str!("../client_secret").trim();

    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("grant_type", "client_credentials"),
        ("scope", "user:read:subscriptions"),
    ];

    let client = reqwest::Client::new();
    let auth = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await?
        .json::<Auth>()
        .await?;

    println!("[{}] starting {auth:?}", Utc::now());

    let _rocket = rocket::build()
        .manage(auth)
        .manage(ClientId { client_id })
        .mount("/", routes![legacy, size])
        .launch()
        .await?;

    Ok(())
}
