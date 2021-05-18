use chrono::Utc;
use rand::Rng;
use tiny_http::{Method, Response, Server};
use url::Url;

const USER_ID_URL: &'static str = "https://api.twitch.tv/helix/users";
const TOKEN_URL: &'static str = "https://id.twitch.tv/oauth2/token";
const DEFAULT_UPPER_LOWER: (i64, i64) = (100, 1);

#[derive(serde::Deserialize)]
struct Users {
    data: Vec<User>,
}

#[derive(serde::Deserialize, PartialEq, Eq, Hash, Debug, Clone)]
struct User {
    id: String,
    display_name: String,
}

#[derive(Debug, serde::Deserialize)]
struct Auth {
    access_token: String,
    expires_in: i64,
}

#[derive(Debug)]
struct LastChecked {
    time: chrono::DateTime<Utc>,
    streamer: User,
}

#[derive(serde::Serialize)]
struct SizeResponse {
    size: i64,
    is_message: bool,
    message: &'static str,
}

lazy_static::lazy_static! {
    static ref VIEWERS: dashmap::DashMap<User, Vec<LastChecked>> = dashmap::DashMap::new();
    static ref STREAMERS: dashmap::DashMap<User, (i64, i64)> = dashmap::DashMap::new();
}

fn get_id(
    auth: &Auth,
    username: &str,
    client_id: &str,
) -> Result<reqwest::blocking::Response, (&'static str, u16)> {
    reqwest::blocking::Client::new()
        .get(
            Url::parse_with_params(USER_ID_URL, &[("login", username)])
                // panic safety: username is validated
                .unwrap()
                .as_str(),
        )
        .header("Authorization", &format!("Bearer {}", auth.access_token))
        .header("Client-Id", client_id)
        .send()
        .map_err(|_| ("couldn't request user ID from twitch API", 500))
}

fn validate(client_id: &str, auth: &Auth, url: &str) -> Result<(User, User), (&'static str, u16)> {
    // get the query parameters
    let url = Url::parse(&format!("a://a{}", url)).map_err(|_| ("invalid request URL", 500))?;

    let mut viewer: Option<String> = None;
    let mut streamer: Option<String> = None;
    let mut time_limit: Option<i64> = None;

    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "streamer" => streamer = Some(v.parse().map_err(|_| ("invalid streamer name", 400))?),
            "viewer" => viewer = Some(v.parse().map_err(|_| ("invalid viewer name", 400))?),
            "time_limit" => time_limit = Some(v.parse().map_err(|_| ("invalid time limit", 400))?),
            _ => {}
        }
    }

    // viewer and streamer required
    if viewer.is_none() || streamer.is_none() {
        return Err(("viewer and streamer required", 400));
    }

    // viewer and streamer names
    // panic safety: viewer is validated
    let viewer = viewer.unwrap();
    let streamer = streamer.unwrap();

    // get viewer and streamer IDs
    let viewer_id_response = get_id(&auth, &viewer, client_id)?;
    if viewer_id_response.status().is_client_error() {
        return Err(("couldn't request viewer ID from twitch API", 500));
    }
    let viewers = viewer_id_response
        .json::<Users>()
        .map_err(|_| ("invalid JSON from twitch (viewer id)", 500))?;
    let viewer = viewers
        .data
        .get(0)
        .ok_or_else(|| ("no viewer by that name", 400))?;

    let streamer_id_response = get_id(&auth, &streamer, client_id)?;
    if streamer_id_response.status().is_client_error() {
        return Err(("couldn't request streamer ID from twitch API", 500));
    }
    let streamers = streamer_id_response
        .json::<Users>()
        .map_err(|_| ("invalid JSON from twitch (streamer ID)", 500))?;
    let streamer = streamers
        .data
        .get(0)
        .ok_or_else(|| ("no streamer by that name", 400))?;

    if VIEWERS.contains_key(viewer) {
        // the viewer is known
        // panic safety: the viewer exists in the map
        let mut last_checked_streams = VIEWERS.get_mut(viewer).unwrap();
        let last_checked = last_checked_streams
            .value_mut()
            .iter_mut()
            .find(|lc| &lc.streamer == streamer);

        if let Some(last_checked) = last_checked {
            // the viewer has checked their size in this chat before
            if let Some(limit) = time_limit {
                // there is a time limit
                if (Utc::now() - last_checked.time).num_seconds() <= limit {
                    // the user hasn't waited long enough
                    return Err(("try again later :)", 200));
                }
            } else {
                // there is no time limit
            }

            // update the time checked
            last_checked.time = Utc::now();
        } else {
            // the viewer has never checked their size in this chat, but has in others
            last_checked_streams.value_mut().push(LastChecked {
                streamer: streamer.clone(),
                time: Utc::now(),
            });
        }
    } else {
        // the viewer is unknown
        VIEWERS.insert(
            viewer.clone(),
            vec![LastChecked {
                streamer: streamer.clone(),
                time: Utc::now(),
            }],
        );
    }

    Ok((streamer.clone(), viewer.clone()))
}

fn reset(auth: &Auth, client_id: &str, url: &str) -> Result<(), (&'static str, u16)> {
    let url = Url::parse(&format!("a://a{}", url)).map_err(|_| ("invalid request URL", 500))?;

    let mut streamer: Option<String> = None;
    let mut upper = DEFAULT_UPPER_LOWER.0;
    let mut lower = DEFAULT_UPPER_LOWER.1;

    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "streamer" => streamer = Some(v.parse().map_err(|_| ("invalid streamer name", 400))?),
            "upper" => upper = v.parse().map_err(|_| ("invalid upper size", 400))?,
            "lower" => lower = v.parse().map_err(|_| ("invalid lower size", 400))?,
            _ => {}
        }
    }
    if streamer.is_none() {
        return Err(("need streamer name", 400));
    }
    // panic safety: the value is validated
    let streamer = streamer.unwrap();

    let streamer_id_response = get_id(&auth, &streamer, client_id)?;
    if streamer_id_response.status().is_client_error() {
        return Err(("couldn't request streamer ID from twitch API", 500));
    }
    let streamers = streamer_id_response
        .json::<Users>()
        .map_err(|_| ("invalid JSON from twitch (streamer ID)", 500))?;
    let streamer = streamers
        .data
        .get(0)
        .ok_or_else(|| ("no streamer by that name", 400))?;

    println!(
        "[{}] resetting {} to {}, {}",
        Utc::now(),
        streamer.display_name,
        upper,
        lower
    );

    if STREAMERS.contains_key(streamer) {
        // panic safety: the map contains the streamer
        *STREAMERS.get_mut(streamer).unwrap().value_mut() = (upper, lower);
    } else {
        STREAMERS.insert(streamer.clone(), (upper, lower));
    }

    Ok(())
}

fn main() {
    // get twitch api authentication
    let client_id = include_str!("../client_id").trim();
    let client_secret = include_str!("../client_secret").trim();

    // panic safety: it's worked a bunch of times before? lol
    let auth_url = Url::parse_with_params(
        TOKEN_URL,
        &[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("grant_type", "client_credentials"),
            ("scope", "user:read:subscriptions"),
        ],
    )
    .unwrap();

    // panic safety: we don't take any user-facing input, twitch's API is probably stable
    // TODO: try again and back off if we can't connect
    let auth = reqwest::blocking::Client::new()
        .post(auth_url.clone())
        .send()
        .unwrap()
        .json::<Auth>()
        .unwrap();

    // start the server
    println!("[{}] starting - auth={:?}", Utc::now(), auth);

    // panic safety: no user-facing input
    let server = Server::http("0.0.0.0:12002").expect("server");
    let start = Utc::now();

    for request in server.incoming_requests() {
        match request.method() {
            Method::Get if request.url() == "/cs" => {
                let lower = 1;
                let upper = 100;
                let rand: i64 = rand::thread_rng().gen_range(lower, upper + 1);
                println!("[{}] legacy api => {}", Utc::now(), rand);
                request
                    .respond(Response::from_string(format!("{}", rand)).with_status_code(308))
                    .unwrap();
            }

            Method::Get if request.url().starts_with("/size") => {
                match validate(client_id, &auth, request.url()) {
                    Ok((streamer, viewer)) => {
                        let mut value = if let Some(bounds) = STREAMERS.get_mut(&streamer) {
                            bounds
                        } else {
                            STREAMERS.insert(streamer.clone(), DEFAULT_UPPER_LOWER);
                            // panic safety: we know the streamer exists in the map
                            STREAMERS.get_mut(&streamer).unwrap()
                        };
                        let (upper, lower) = value.value_mut();

                        let rand: i64 = rand::thread_rng().gen_range(*lower, *upper + 1);

                        if rand == *lower {
                            *lower = *lower - 1;
                            println!(
                                "[{}] {}: {} is unlucky - new lower bound: {}",
                                Utc::now(),
                                streamer.display_name,
                                viewer.display_name,
                                lower
                            );
                        } else if rand == *upper {
                            *upper = *upper + 1;
                            println!(
                                "[{}] {}: {} is blessed - new upper bound: {}",
                                Utc::now(),
                                streamer.display_name,
                                viewer.display_name,
                                upper
                            );
                        }

                        println!(
                            "[{}] {}: {} got {}",
                            Utc::now(),
                            streamer.display_name,
                            viewer.display_name,
                            rand
                        );

                        // panic safety: no custom serialization impl
                        // panic safety: TODO: do nothing if we can't respond
                        request
                            .respond(Response::from_string(
                                serde_json::to_string(&SizeResponse {
                                    size: rand,
                                    is_message: false,
                                    message: "",
                                })
                                .unwrap(),
                            ))
                            .unwrap()
                    }

                    // panic safety: no custom serialization impl
                    // panic safety: TODO: do nothing if we can't respond
                    Err((body, status)) => request
                        .respond(
                            Response::from_string(
                                serde_json::to_string(&SizeResponse {
                                    size: status as i64,
                                    is_message: true,
                                    message: body,
                                })
                                .unwrap(),
                            )
                            .with_status_code(status),
                        )
                        .unwrap(),
                }
            }

            Method::Get if request.url() == "/up" => {
                let diff = Utc::now() - start;

                let days = diff.num_days();
                let hours = diff.num_hours() % 24;
                let minutes = diff.num_minutes() % 60;
                let seconds = diff.num_seconds() % 60;

                // panic safety: TODO: do nothing if we can't respond
                request
                    .respond(Response::from_string(format!(
                        "{}d {}h {}m {}s",
                        days, hours, minutes, seconds,
                    )))
                    .expect("response /up");
            }

            Method::Post if request.url() == "/poweroff" => {
                println!("[{}] shutting down!", Utc::now());
                break;
            }

            Method::Put if request.url().starts_with("/reset") => {
                match reset(&auth, client_id, request.url()) {
                    // panic safety: no custom serialization impl
                    // panic safety: TODO: do nothing if we can't respond
                    Ok(()) => request
                        .respond(Response::from_string(
                            serde_json::to_string(&SizeResponse {
                                size: 0,
                                is_message: true,
                                message: "reset size",
                            })
                            .unwrap(),
                        ))
                        .unwrap(),

                    // panic safety: no custom serialization impl
                    // panic safety: TODO: do nothing if we can't respond
                    Err((body, status)) => request
                        .respond(
                            Response::from_string(
                                serde_json::to_string(&SizeResponse {
                                    size: status as i64,
                                    is_message: true,
                                    message: body,
                                })
                                .unwrap(),
                            )
                            .with_status_code(status),
                        )
                        .unwrap(),
                }
            }

            // panic safety: TODO: do nothing if we can't respond
            _ => request
                .respond(Response::from_string("Invalid request!").with_status_code(400))
                .expect("error message"),
        }
    }
}

#[cfg(test)]
#[test]
fn test() {
    // this could still fail, but it's super unlikely

    let mut lower = 1;
    let mut upper = 10;

    for _ in 0..10000 {
        let rand: i64 = rand::thread_rng().gen_range(lower, upper + 1);

        if rand == lower {
            lower -= 1;
        }

        if rand == upper {
            upper += 1;
        }
    }

    assert_ne!(lower, 1);
    assert_ne!(upper, 10);
}
