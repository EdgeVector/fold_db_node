use crate::commands::CommandOutput;

pub fn render(output: &CommandOutput) {
    match output {
        CommandOutput::Message(msg) => {
            println!("{}", msg);
        }

        CommandOutput::RawJson(json) => {
            println!(
                "{}",
                serde_json::to_string_pretty(json).unwrap_or_else(|_| json.to_string())
            );
        }

        CommandOutput::Completions(script) => {
            print!("{}", script);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_message() {
        render(&CommandOutput::Message("test message".to_string()));
    }

    #[test]
    fn human_completions() {
        render(&CommandOutput::Completions("# completions".into()));
    }
}
