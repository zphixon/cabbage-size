#[macro_use]
extern crate rocket;

use std::collections::{hash_map::Entry, HashMap};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use rand::Rng;
use rocket::{http::Status, response::status::Custom, serde::json::Json, State};
use serde::{Deserialize, Serialize};

const USER_ID_URL: &'static str = "https://api.twitch.tv/helix/users";
const TOKEN_URL: &'static str = "https://id.twitch.tv/oauth2/token";

static VIEWERS: Lazy<DashMap<User, Vec<LastChecked>>> = Lazy::new(|| DashMap::new());
static STREAMERS: Lazy<DashMap<User, ChannelStatus>> = Lazy::new(|| DashMap::new());
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

impl std::fmt::Display for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name)
    }
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
    time: DateTime<Utc>,
    limit: Option<i64>,
    streamer: User,
    size: i64,
}

#[derive(Serialize)]
struct Size {
    size: i64,
    is_message: bool,
    message: &'static str,
}

#[derive(Debug)]
struct BoonLength {
    time: DateTime<Utc>,
    length: i64,
}

#[derive(Debug)]
enum BoonKind {
    Cursed,
    Blessed,
}

#[derive(Debug)]
struct Boon {
    kind: BoonKind,
    value: Option<i64>,
    duration: Option<BoonLength>,
}

#[derive(Debug, Default)]
struct ChannelStatus {
    boons: HashMap<User, Boon>,
    active_boon: Option<Boon>,
    bounds: Bounds,
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

type SizeResponse = Custom<Json<Size>>;

fn ok_response(size: i64) -> SizeResponse {
    Custom(
        Status::Ok,
        Json(Size {
            size,
            is_message: false,
            message: "",
        }),
    )
}

fn make_response(status: Status, message: &'static str) -> SizeResponse {
    Custom(
        status,
        Json(Size {
            size: status.code as i64,
            is_message: true,
            message,
        }),
    )
}

async fn get_user(
    auth: &State<Auth>,
    client_id: &State<ClientId>,
    user: &str,
) -> Result<User, SizeResponse> {
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
        Err(e) => {
            dbg!(e);
            return Err(make_response(
                Status::InternalServerError,
                "twitch api request failed",
            ));
        }
    };

    let users = user_response.json::<Users>().await;
    let mut users = match users {
        Ok(users) => users,
        Err(e) => {
            dbg!(e);
            return Err(make_response(
                Status::InternalServerError,
                "twitch json response nonsensical",
            ));
        }
    };

    // all of that was just getting the user in json form
    if users.data.is_empty() {
        Err(make_response(
            Status::InternalServerError,
            "twitch returned no users",
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
) -> SizeResponse {
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
    let mut last_checked_streams = VIEWERS.entry(viewer.clone()).or_default();
    if let Some(last_checked) = last_checked_streams
        .iter_mut()
        .find(|lc| lc.streamer == streamer)
    {
        // we have checked our size in this streamer's chat before
        if let Some(limit) = time_limit {
            // there is a time limit
            if (Utc::now() - last_checked.time).num_seconds() <= limit {
                // the time limit has not elapsed - do not update the size, just return it
                return ok_response(last_checked.size);
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
    let mut channel_status = STREAMERS.entry(streamer).or_default();
    let size = if let Entry::Occupied(boon_entry) = channel_status.boons.entry(viewer.clone()) {
        let boon = boon_entry.get();
        let elapsed = if let Some(duration) = &boon.duration {
            (Utc::now() - duration.time).num_seconds() <= duration.length
        } else {
            true
        };

        if elapsed {
            let _ = boon_entry.remove();
            make_size(&mut channel_status.bounds)
        } else {
            match boon.kind {
                BoonKind::Cursed => boon.value.unwrap_or(channel_status.bounds.lower),
                BoonKind::Blessed => boon.value.unwrap_or(channel_status.bounds.upper),
            }
        }
    } else if channel_status.active_boon.is_some() {
        let boon = channel_status.active_boon.as_ref().unwrap();
        let elapsed = if let Some(duration) = &boon.duration {
            (Utc::now() - duration.time).num_seconds() <= duration.length
        } else {
            true
        };

        if elapsed {
            channel_status.active_boon = None;
            make_size(&mut channel_status.bounds)
        } else {
            match boon.kind {
                BoonKind::Cursed => boon.value.unwrap_or(channel_status.bounds.lower),
                BoonKind::Blessed => boon.value.unwrap_or(channel_status.bounds.upper),
            }
        }
    } else {
        make_size(&mut channel_status.bounds)
    };

    lc.size = size;

    ok_response(size)
}

#[put("/reset?<streamer>&<upper>&<lower>")]
async fn change_bounds(
    streamer: &str,
    upper: Option<i64>,
    lower: Option<i64>,
    auth: &State<Auth>,
    client_id: &State<ClientId>,
) -> SizeResponse {
    // there must be a better way to do this
    let streamer = match get_user(auth, client_id, streamer).await {
        Ok(streamer) => streamer,
        Err(response) => return response,
    };

    // set the new bounds
    let mut channel_status = STREAMERS.entry(streamer).or_default();
    channel_status.bounds.upper = upper.unwrap_or(100);
    channel_status.bounds.lower = lower.unwrap_or(1);

    make_response(Status::Ok, "size reset")
}

#[get("/bless?<viewer>&<streamer>&<value>&<time_limit>")]
async fn bless(
    streamer: &str,
    viewer: Option<&str>,
    value: Option<i64>,
    time_limit: Option<i64>,
    auth: &State<Auth>,
    client_id: &State<ClientId>,
) -> SizeResponse {
    let duration = time_limit.map(|length| BoonLength {
        time: Utc::now(),
        length,
    });

    let boon = Boon {
        kind: BoonKind::Blessed,
        value,
        duration,
    };

    let streamer = match get_user(auth, client_id, streamer).await {
        Ok(streamer) => streamer,
        Err(response) => return response,
    };

    let mut channel_status = STREAMERS.entry(streamer).or_default();

    if let Some(viewer) = viewer {
        let viewer = match get_user(auth, client_id, viewer).await {
            Ok(viewer) => viewer,
            Err(response) => return response,
        };

        channel_status.boons.insert(viewer, boon);
    } else {
        channel_status.active_boon = Some(boon);
    }

    make_response(Status::Ok, "user blessed")
}

#[get("/curse?<viewer>&<streamer>&<value>&<time_limit>")]
fn curse(
    streamer: &str,
    viewer: Option<&str>,
    value: Option<i64>,
    time_limit: Option<i64>,
) -> SizeResponse {
    let _ = streamer;
    let _ = viewer;
    let _ = value;
    let _ = time_limit;
    make_response(Status::Ok, "user cursed")
}

#[get("/up")]
fn uptime(start: &State<DateTime<Utc>>) -> String {
    // deref go brrr
    let diff = Utc::now() - **start;
    let days = diff.num_days();
    let hours = diff.num_hours() % 24;
    let minutes = diff.num_minutes() % 60;
    let seconds = diff.num_seconds() % 60;
    format!("{days}d {hours}h {minutes}m {seconds}s")
}

#[post("/clean")]
fn clean_viewers() -> SizeResponse {
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

    make_response(Status::Ok, "viewers purged")
}

#[get("/status")]
fn status() -> String {
    let mut s = String::from("users:\n");
    for entry in VIEWERS.iter() {
        let viewer = entry.key();
        let last_checked = entry.value();
        s += &format!("\t{}\n", viewer);
        for check in last_checked {
            s += &format!("\t\t{} => {}", check.streamer, check.size);
            if let Some(limit) = check.limit {
                s += &format!(", refresh in {}s after {}", limit, check.time);
            }
            s += "\n";
        }
    }
    s += "streamers:\n";
    for entry in STREAMERS.iter() {
        let streamer = entry.key();
        let status = entry.value();
        s += &format!("\t{streamer} => {:?}", status.bounds);
        if let Some(boon) = &status.active_boon {
            if let Some(duration) = &boon.duration {
                s += &format!(
                    " (users are {:?} until {}s after {})",
                    boon.kind, duration.length, duration.time
                );
            } else {
                s += &format!(" (next user is {:?})", boon.kind);
            }
        }
        s += "\n";
        for (user, boon) in &status.boons {
            s += &format!("\t\t{user}: {:?}\n", boon.kind);
        }
    }
    s + "\n"
}

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
            routes![
                legacy,
                size,
                uptime,
                change_bounds,
                status,
                clean_viewers,
                bless,
                curse,
            ],
        )
        .launch()
        .await?;

    Ok(())
}
