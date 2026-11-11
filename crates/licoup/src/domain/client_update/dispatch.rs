use anyhow::{Result, bail};
use serde_json::Value;

pub fn dispatch(args: &[String], params: &Value) -> Result<Value> {
    let github = params.get("source").and_then(Value::as_str).map(str::trim) == Some("github");
    match args.get(1).map(String::as_str).unwrap_or("status") {
        "status" => {
            if github {
                super::github_source::status_github(params)
            } else {
                super::status::status(params)
            }
        }
        "check" => {
            if github {
                super::github_source::check_github(params)
            } else {
                super::check::check(params)
            }
        }
        "download" => {
            if github {
                super::github_source::download_github(params)
            } else {
                super::download::download(params)
            }
        }
        "verify" => {
            let effective = github_context(params)?;
            super::verify::verify(&effective)
        }
        "apply" => {
            let effective = github_context(params)?;
            super::apply::apply(&effective)
        }
        _ => bail!("client update command is unsupported"),
    }
}

fn github_context(params: &Value) -> Result<Value> {
    if params.get("source").and_then(Value::as_str).map(str::trim) == Some("github") {
        super::github_source::github_context_params(params)
    } else {
        Ok(params.clone())
    }
}
