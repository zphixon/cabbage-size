use rand::Rng;
use tiny_http::{Method, Response, Server};

use std::fs::{read_to_string, File};
use std::io::Write;

fn main() {
    let upper = read_to_string("upper_cabbage_size").expect("upper file");
    let lower = read_to_string("lower_cabbage_size").expect("lower file");

    let mut upper = upper.trim().parse::<i64>().expect("parse upper");
    let mut lower = lower.trim().parse::<i64>().expect("parse lower");

    println!("[{}] starting", chrono::Utc::now());
    let server = Server::http("0.0.0.0:8080").expect("server");

    for request in server.incoming_requests() {
        match request.method() {
            Method::Get if request.url() == "/cs" => {
                let rand: i64 = rand::thread_rng().gen_range(lower, upper + 1);

                if rand == lower {
                    lower -= 1;
                    println!("[{}] new lower bound: {}", chrono::Utc::now(), lower);
                }

                if rand == upper {
                    upper += 1;
                    println!("[{}] new upper bound: {}", chrono::Utc::now(), upper);
                }

                request
                    .respond(Response::from_string(format!("{}", rand)))
                    .expect("response");
            }

            Method::Post if request.url() == "/po" => {
                println!("[{}] shutting down!", chrono::Utc::now());
                break;
            }

            _ => request
                .respond(Response::from_string("Invalid request!").with_status_code(400))
                .expect("error message"),
        }
    }

    let mut upper_file = File::create("upper_cabbage_size").expect("create upper file");
    let mut lower_file = File::create("lower_cabbage_size").expect("create lower file");
    upper_file
        .write_all(format!("{}", upper).as_bytes())
        .expect("write upper file");
    lower_file
        .write_all(format!("{}", lower).as_bytes())
        .expect("write lower file");
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
