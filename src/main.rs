#[macro_use]
extern crate rocket;

use std::collections::hash_map::Entry;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use rand::Rng;
use rocket::{http::Status, State};

use cs::{
    make_response, ok_response, Auth, Boon, BoonKind, BoonLength, Bounds, ChannelStatus, ClientId,
    LastChecked, SizeResponse, User, Users, TOKEN_URL, USER_ID_URL,
};

static VIEWERS: Lazy<DashMap<User, Vec<LastChecked>>> = Lazy::new(|| DashMap::new());
static STREAMERS: Lazy<DashMap<User, ChannelStatus>> = Lazy::new(|| DashMap::new());
static USER_CACHE: Lazy<DashMap<String, User>> = Lazy::new(|| DashMap::new());

#[get("/cs")]
fn legacy() -> String {
    let rand: i64 = rand::thread_rng().gen_range(1..=100);
    format!("{rand}")
}

async fn get_user(
    auth: &State<Auth>,
    client_id: &State<ClientId>,
    user: &str,
) -> Result<User, SizeResponse> {
    // if we already know who this user is don't bother checking again
    if let Some(user_data) = USER_CACHE.get(user) {
        rocket::debug!("user {user} cached => {:?}", *user_data);
        return Ok(user_data.clone());
    }

    rocket::debug!("looking for {user} from twitch api");
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
        Err(ref e) => {
            rocket::error!("{user_response:?} {e:?}");
            return Err(make_response(
                Status::InternalServerError,
                "twitch api request failed",
            ));
        }
    };

    let raw = user_response
        .text()
        .await
        .unwrap_or_else(|err| format!("{err:?}"));
    let users = rocket::serde::json::from_str::<Users>(&raw);
    let mut users = match users {
        Ok(users) => users,
        Err(e) => {
            rocket::error!("{raw} {e:?}");
            return Err(make_response(
                Status::InternalServerError,
                "twitch json response nonsensical",
            ));
        }
    };

    // all of that was just getting the user in json form
    if users.data.is_empty() {
        rocket::error!("no user named {user}");
        Err(make_response(
            Status::InternalServerError,
            "twitch returned no users",
        ))
    } else {
        // throw 'em on the pile
        let user_data = users.data.pop().unwrap();
        rocket::debug!("found user {user} => {:?}", user_data);

        USER_CACHE.insert(user.into(), user_data.clone());
        Ok(user_data)
    }
}

fn make_size(bounds: &mut Bounds) -> i64 {
    let Bounds { upper, lower } = bounds;
    let rand: i64 = rand::thread_rng().gen_range(*lower..=*upper);
    if rand == *lower {
        rocket::debug!("generated {rand}, new lower bound");
        *lower -= 1;
    } else if rand == *upper {
        rocket::debug!("generated {rand}, new upper bound");
        *upper += 1;
    } else {
        rocket::debug!("generated {rand}");
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
                rocket::info!(
                    "{streamer}: {viewer} should try again {limit}s after {}",
                    last_checked.time
                );
                // the time limit has not elapsed - do not update the size, just return it
                return ok_response(last_checked.size);
            } else {
                rocket::debug!("{streamer}: {viewer} time limit of {limit}s passed");
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
        rocket::debug!("{streamer}: {viewer} is new, can get a new size in {time_limit:?}s");
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
    let mut channel_status = STREAMERS.entry(streamer.clone()).or_default();
    let bounds = channel_status.bounds;

    use BoonCalculation::*;
    #[derive(Debug)]
    enum BoonCalculation {
        Predetermined(i64),
        Random,
    }
    let mut boon_calculation = Random;

    if let Entry::Occupied(viewer_boon_entry) = channel_status.boons.entry(viewer.clone()) {
        let viewer_boon = viewer_boon_entry.get();
        if let Some(value) = viewer_boon.value {
            boon_calculation = Predetermined(value);
        } else {
            boon_calculation = Predetermined(match viewer_boon.kind {
                BoonKind::Blessed => bounds.upper,
                BoonKind::Cursed => bounds.lower,
            });
        }

        if let Some(duration) = &viewer_boon.duration {
            if !duration.active() {
                rocket::info!(
                    "{streamer}: {viewer}'s {:?} status is expired",
                    viewer_boon.kind
                );
                boon_calculation = Random;
                viewer_boon_entry.remove();
            } else {
                rocket::info!(
                    "{streamer}: {viewer}'s {:?} status remains.",
                    viewer_boon.kind
                );
            }
        } else {
            rocket::info!(
                "{streamer}: {viewer}'s {:?} status has been consumed",
                viewer_boon.kind
            );
            viewer_boon_entry.remove();
        }
    } else if let Some(active_boon) = channel_status.active_boon {
        if let Some(value) = active_boon.value {
            boon_calculation = Predetermined(value);
        } else {
            boon_calculation = Predetermined(match active_boon.kind {
                BoonKind::Blessed => bounds.upper,
                BoonKind::Cursed => bounds.lower,
            });
        }

        if let Some(duration) = &active_boon.duration {
            if !duration.active() {
                rocket::info!(
                    "{streamer}: chat's {:?} status is expired",
                    active_boon.kind
                );
                boon_calculation = Random;
                channel_status.active_boon = None;
            } else {
                rocket::info!(
                    "{streamer}: the chat's {:?} status remains.",
                    active_boon.kind
                );
            }
        } else {
            rocket::info!(
                "{streamer}: chat's {:?} has been consumed",
                active_boon.kind
            );
            channel_status.active_boon = None;
        }
    }

    let size = match boon_calculation {
        Predetermined(size) => size,
        Random => make_size(&mut channel_status.bounds),
    };

    lc.size = size;
    rocket::info!("{streamer}: {viewer} got size {size}");

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

    let mut channel_status = STREAMERS.entry(streamer.clone()).or_default();

    if let Some(viewer) = viewer {
        let viewer = match get_user(auth, client_id, viewer).await {
            Ok(viewer) => viewer,
            Err(response) => return response,
        };

        rocket::info!("{streamer}: {viewer} has been blessed! {boon:?}");
        channel_status.boons.insert(viewer, boon);
    } else {
        rocket::info!("{streamer}: chat has been blessed! {boon:?}");
        channel_status.active_boon = Some(boon);
    }

    make_response(Status::Ok, "user blessed")
}

#[get("/curse?<viewer>&<streamer>&<value>&<time_limit>")]
async fn curse(
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
        kind: BoonKind::Cursed,
        value,
        duration,
    };

    let streamer = match get_user(auth, client_id, streamer).await {
        Ok(streamer) => streamer,
        Err(response) => return response,
    };

    let mut channel_status = STREAMERS.entry(streamer.clone()).or_default();

    if let Some(viewer) = viewer {
        let viewer = match get_user(auth, client_id, viewer).await {
            Ok(viewer) => viewer,
            Err(response) => return response,
        };

        rocket::info!("{streamer}: {viewer} has been cursed! {boon:?}");
        channel_status.boons.insert(viewer, boon);
    } else {
        rocket::info!("{streamer}: chat has been cursed! {boon:?}");
        channel_status.active_boon = Some(boon);
    }

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
    env_logger::init();
    let args = std::env::args().collect::<Vec<_>>();
    let contents = std::fs::read_to_string(args.get(1).expect("need config filename"))
        .expect("couldn't read config");
    let toml = contents
        .parse::<toml_edit::Document>()
        .expect("invalid toml");

    let client_id = String::from(toml["client_id"].as_str().expect("need client_id str"));
    let client_secret = String::from(
        toml["client_secret"]
            .as_str()
            .expect("need client_secret str"),
    );

    // get twitch authorization
    let params = [
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("grant_type", "client_credentials"),
        ("scope", "user:read:subscriptions"),
    ];
    println!("{params:?}");
    println!(
        "{}",
        reqwest::Url::parse_with_params(TOKEN_URL, params).unwrap()
    );

    let client = reqwest::Client::new();
    let auth = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await?
        .text()
        .await?;

    println!("{}", auth);
    let auth = rocket::serde::json::from_str::<Auth>(&auth)?;

    println!("[{}] starting {auth:?}", Utc::now());

    // start the server - don't forget to add the routes here
    let _rocket = rocket::build()
        .manage(auth)
        .manage(ClientId {
            client_id: Box::leak(client_id.into_boxed_str()),
        })
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
