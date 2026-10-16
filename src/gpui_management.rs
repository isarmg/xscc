//! Typed management operations for native UI callers; authorization stays with the owner.
use hbb_common::ResultType;
use xscc::protocol::HostAddress;
use xscc_native_management::manager::Request;
pub use xscc_native_management::manager::Status;

pub enum Command {
    Enroll {
        server: String,
        authorization_code: String,
        addresses: Vec<HostAddress>,
    },
    Resume,
    Unbind,
    SetAddresses {
        addresses: Vec<HostAddress>,
    },
}

impl Command {
    fn into_request(self) -> Request {
        match self {
            Self::Enroll {
                server,
                authorization_code,
                addresses,
            } => Request::Enroll {
                server,
                authorization_code,
                addresses,
            },
            Self::Resume => Request::Resume {},
            Self::Unbind => Request::Unbind {},
            Self::SetAddresses { addresses } => Request::SetAddresses { addresses },
        }
    }
}

pub async fn status() -> ResultType<Status> {
    request_status(Request::Status {}).await
}

pub async fn execute(command: Command) -> ResultType<Status> {
    request_status(command.into_request()).await
}

async fn request_status(request: Request) -> ResultType<Status> {
    Ok(serde_json::from_value(
        crate::management::rpc(request).await?,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrollment_forwards_server_credentials_and_addresses_to_the_owner() {
        let request = Command::Enroll {
            server: "https://example.invalid".into(),
            authorization_code: "server-issued-code".into(),
            addresses: vec![HostAddress {
                host: "host.example.invalid".into(),
                port: 21118,
            }],
        }
        .into_request();
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "type": "enroll", "server": "https://example.invalid",
                "authorization_code": "server-issued-code",
                "addresses": [{"host": "host.example.invalid", "port": 21118}]
            })
        );
    }

    #[test]
    fn management_commands_preserve_owner_request_contract() {
        for (command, expected) in [
            (Command::Resume, serde_json::json!({"type": "resume"})),
            (Command::Unbind, serde_json::json!({"type": "unbind"})),
            (
                Command::SetAddresses { addresses: vec![] },
                serde_json::json!({"type": "set_addresses", "addresses": []}),
            ),
        ] {
            assert_eq!(
                serde_json::to_value(command.into_request()).unwrap(),
                expected
            );
        }
    }
}
