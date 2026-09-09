use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "message")]
pub enum Request {
    Ping,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "message")]
pub enum Response {
    Pong,
}

pub fn handle(req: &Request) -> Response {
    match req {
        Request::Ping => Response::Pong,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{Request, Response, handle};

    #[test]
    fn ping_is_answered_with_pong() {
        assert!(matches!(handle(&Request::Ping), Response::Pong));
    }

    /// The wire contract between two Astrofin builds. A second instance from
    /// another release has to keep parsing exactly this.
    #[test]
    fn the_wire_form_is_one_tagged_object() {
        assert_eq!(
            serde_json::to_string(&Request::Ping).unwrap(),
            r#"{"message":"Ping"}"#
        );
        assert_eq!(
            serde_json::to_string(&Response::Pong).unwrap(),
            r#"{"message":"Pong"}"#
        );
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"message":"Ping"}"#).unwrap(),
            Request::Ping
        ));
        assert!(matches!(
            serde_json::from_str::<Response>(r#"{"message":"Pong"}"#).unwrap(),
            Response::Pong
        ));
    }

    /// Anything a local process can put on the wire that is not the contract
    /// is refused, never guessed at.
    #[test]
    fn a_request_that_is_not_ping_is_refused() {
        for body in [
            r#"{"message":"Pong"}"#,
            r#"{"message":"ping"}"#,
            r#"{"message":"Ping "}"#,
            r#"{"message":42}"#,
            r#"{"message":null}"#,
            r#"{"Message":"Ping"}"#,
            "{}",
            "[]",
            "null",
            "\"Ping\"",
            "",
        ] {
            assert!(serde_json::from_str::<Request>(body).is_err(), "{body}");
        }
    }

    /// Extra keys are ignored, which is what lets a newer instance talk to an
    /// older one.
    #[test]
    fn a_request_with_extra_keys_still_parses() {
        let req: Request =
            serde_json::from_str(r#"{"message":"Ping","extra":{"a":[1,2,3]}}"#).unwrap();
        assert!(matches!(req, Request::Ping));
    }
}
