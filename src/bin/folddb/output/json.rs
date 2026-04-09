use crate::commands::CommandOutput;
use serde_json::{json, Value};

pub fn render(output: &CommandOutput) {
    let val = to_json(output);
    println!(
        "{}",
        serde_json::to_string(&val)
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
    );
}

fn to_json(output: &CommandOutput) -> Value {
    match output {
        CommandOutput::Message(msg) => {
            json!({ "ok": true, "message": msg })
        }
        CommandOutput::RawJson(json) => {
            // Pass through daemon response directly
            json.clone()
        }
        CommandOutput::Completions(_) => {
            json!({ "ok": true, "message": "Completions written to stdout" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandOutput;

    fn assert_ok(output: &CommandOutput) {
        let val = to_json(output);
        assert_eq!(val["ok"], true, "Expected ok:true for {:?}", output);
    }

    #[test]
    fn json_message() {
        assert_ok(&CommandOutput::Message("test message".to_string()));
    }

    #[test]
    fn json_completions() {
        assert_ok(&CommandOutput::Completions("# bash completions".into()));
    }
}
