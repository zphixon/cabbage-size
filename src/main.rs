// vim: set nu rnu sw=4 et ai si

#[macro_use]
extern crate rocket;

use chrono::Utc;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use rand::Rng;
use rocket::{http::Status, response::status::Custom as Response, serde::json::Json, State};
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

type MyResponse = Response<Json<SizeResponse>>;

async fn get_user(
    auth: &State<Auth>,
    client_id: &State<ClientId>,
    user: &str,
) -> Result<User, MyResponse> {
    // if we already know who this user is don't bother checking again
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

    // there's some way to implement your own rocket responses
    // but i don't have the patience rn
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

    // all of that was just getting the user in json form
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
        // throw 'em on the pile
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
) -> MyResponse {
    // get the viewer and streamer twitch info (to verify a user exists)
    let viewer = match get_user(auth, client_id, viewer).await {
        Ok(viewer) => viewer,
        Err(response) => return response,
    };

    let streamer = match get_user(auth, client_id, streamer).await {
        Ok(streamer) => streamer,
        Err(response) => return response,
    };

    // this is hacky wheee - get the times a user has last checked their size
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

        // and also update the time and new limit if it exists
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

    // get the new size
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

#[put("/reset?<streamer>&<upper>&<lower>")]
async fn change_bounds(
    streamer: &str,
    upper: Option<i64>,
    lower: Option<i64>,
    auth: &State<Auth>,
    client_id: &State<ClientId>,
) -> MyResponse {
    // there must be a better way to do this
    let streamer = match get_user(auth, client_id, streamer).await {
        Ok(streamer) => streamer,
        Err(response) => return response,
    };

    // set the new bounds
    let mut bounds = STREAMERS.entry(streamer).or_default();
    bounds.upper = upper.unwrap_or(100);
    bounds.lower = lower.unwrap_or(1);

    Response(
        Status::Ok,
        Json(SizeResponse {
            size: 200,
            is_message: true,
            message: "size reset",
        }),
    )
}

#[get("/up")]
fn uptime(start: &State<chrono::DateTime<Utc>>) -> String {
    // deref go brrr
    let diff = Utc::now() - **start;
    let days = diff.num_days();
    let hours = diff.num_hours() % 24;
    let minutes = diff.num_minutes() % 60;
    let seconds = diff.num_seconds() % 60;
    format!("{days}d {hours}h {minutes}m {seconds}s")
}

#[post("/clean")]
fn clean_viewers() -> MyResponse {
    let now = Utc::now();

    // for every viewer
    VIEWERS.iter_mut().for_each(|mut rmm| {
        // go through last checked
        rmm.value_mut().retain(|check| {
            // if the checked time is elapsed remove it
            check.limit.is_some() && (now - check.time).num_seconds() <= check.limit.unwrap()
        })
    });

    // if they don't have any checked times remove them
    VIEWERS.retain(|_, checks| !checks.is_empty());

    Response(
        Status::Ok,
        Json(SizeResponse {
            size: 200,
            is_message: true,
            message: "viewers purged",
        }),
    )
}

#[get("/status")]
fn status() -> String {
    let mut s = String::from("\n");
    for entry in VIEWERS.iter() {
        let viewer = entry.key();
        let lc = entry.value();
        s += &format!("{viewer:?} => {lc:?}\n");
    }
    for entry in STREAMERS.iter() {
        let streamer = entry.key();
        let bounds = entry.value();
        s += &format!("{streamer:?} => {bounds:?}\n");
    }
    s
}

// TODO:
// add bless/curse - makes the next /size request the upper or lower bound
//                 - for a specific user or any user, maybe with a specific value

#[rocket::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // TODO make this not part of the binary lol
    let client_id = include_str!("../client_id").trim();
    let client_secret = include_str!("../client_secret").trim();

    // get twitch authorization
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

    // start the server - don't forget to add the routes here
    let _rocket = rocket::build()
        .manage(auth)
        .manage(ClientId { client_id })
        .manage(Utc::now())
        .mount(
            "/",
            routes![legacy, size, uptime, change_bounds, status, clean_viewers,],
        )
        .launch()
        .await?;

    Ok(())
}
