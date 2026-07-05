const DISPATCH_326: &[DispatcherEntry] = &[
    DispatcherEntry { command: "grok", handler: Handler::Async(grok_run_async) }
];

const GROK_USAGE: &str = "usage: maw grok <subcommand> [args]
  login                     authenticate via xAI OIDC device flow
  status                    check OIDC token credentials status
  gen <prompt> [ratio]      generate still image from text prompt (aspect_ratio: 16:9, 9:16, 1:1)
  video <prompt> [img_path] animate image/text into 6s/10s video (async polling)
";

fn grok_run_async(args: Vec<String>) -> std::pin::Pin<Box<dyn std::future::Future<Output = CliOutput> + Send>> {
    Box::pin(async move { grok_dispatch(&args).await })
}

async fn grok_dispatch(args: &[String]) -> CliOutput {
    if args.is_empty() {
        return CliOutput { code: 1, stdout: String::new(), stderr: GROK_USAGE.to_owned() };
    }
    let cmd = args[0].as_str();
    match cmd {
        "help" | "--help" | "-h" => CliOutput { code: 0, stdout: GROK_USAGE.to_owned(), stderr: String::new() },
        "login" => grok_login(),
        "status" => grok_status(),
        "gen" => {
            if args.len() < 2 {
                return CliOutput { code: 1, stdout: String::new(), stderr: "usage: maw grok gen <prompt> [aspect_ratio]\n".to_owned() };
            }
            let prompt = &args[1];
            let ratio = args.get(2).map(String::as_str).unwrap_or("16:9");
            grok_gen_image(prompt, ratio).await
        },
        "video" => {
            if args.len() < 2 {
                return CliOutput { code: 1, stdout: String::new(), stderr: "usage: maw grok video <prompt> [image_path]\n".to_owned() };
            }
            let prompt = &args[1];
            let img_path = args.get(2).map(String::as_str);
            grok_gen_video(prompt, img_path).await
        },
        _ => CliOutput { code: 1, stdout: String::new(), stderr: format!("unknown subcommand '{cmd}'\n\n{GROK_USAGE}") },
    }
}

fn grok_login() -> CliOutput {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/admin".to_owned());
    let grok_bin = std::path::PathBuf::from(home).join(".grok/bin/grok");
    let binary = if grok_bin.exists() {
        grok_bin.to_string_lossy().to_string()
    } else {
        "grok".to_owned()
    };
    
    let status = std::process::Command::new(binary)
        .arg("login")
        .arg("--device-auth")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();
        
    match status {
        Ok(status) if status.success() => CliOutput {
            code: 0,
            stdout: "grok: login successful!\n".to_owned(),
            stderr: String::new(),
        },
        Ok(status) => CliOutput {
            code: status.code().unwrap_or(1),
            stdout: String::new(),
            stderr: format!("grok: login failed with exit code {}\n", status.code().unwrap_or(1)),
        },
        Err(e) => CliOutput {
            code: 1,
            stdout: String::new(),
            stderr: format!("grok: failed to run grok login command: {e}\n"),
        },
    }
}

fn grok_auth_path() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = std::path::PathBuf::from(home).join(".grok/auth.json");
        if p.exists() {
            return p;
        }
    }
    let p = std::path::PathBuf::from("/Users/admin/.grok/auth.json");
    if p.exists() {
        return p;
    }
    let p = std::path::PathBuf::from("/root/.grok/auth.json");
    if p.exists() {
        return p;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/admin".to_owned());
    std::path::PathBuf::from(home).join(".grok/auth.json")
}

fn grok_status() -> CliOutput {
    let auth_path = grok_auth_path();
    if !auth_path.exists() {
        return CliOutput {
            code: 1,
            stdout: String::new(),
            stderr: format!(
                "grok: no active session found at {}. Please run 'maw grok login' to authenticate.\n",
                auth_path.display()
            ),
        };
    }
    
    let raw = match std::fs::read_to_string(&auth_path) {
        Ok(s) => s,
        Err(e) => return CliOutput {
            code: 1,
            stdout: String::new(),
            stderr: format!("grok: failed to read auth.json at {}: {e}\n", auth_path.display()),
        },
    };
    
    let json: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return CliOutput {
            code: 1,
            stdout: String::new(),
            stderr: format!("grok: failed to parse auth.json: {e}\n"),
        },
    };
    
    let obj = match json.as_object() {
        Some(o) => o,
        None => return CliOutput {
            code: 1,
            stdout: String::new(),
            stderr: "grok: invalid auth.json structure\n".to_owned(),
        },
    };
    
    if obj.is_empty() {
        return CliOutput {
            code: 1,
            stdout: String::new(),
            stderr: "grok: auth.json contains no credentials. Run 'maw grok login' first.\n".to_owned(),
        };
    }
    
    let (key, val) = obj.iter().next().unwrap();
    let email = val.get("email").and_then(serde_json::Value::as_str).unwrap_or("unknown");
    let first_name = val.get("first_name").and_then(serde_json::Value::as_str).unwrap_or("");
    let last_name = val.get("last_name").and_then(serde_json::Value::as_str).unwrap_or("");
    let expires_at = val.get("expires_at").and_then(serde_json::Value::as_str).unwrap_or("unknown");
    let team_id = val.get("team_id").and_then(serde_json::Value::as_str).unwrap_or("unknown");
    
    CliOutput {
        code: 0,
        stdout: format!(
            "grok session status:\n  email      {email}\n  user       {first_name} {last_name}\n  team_id    {team_id}\n  expires_at {expires_at}\n  source     {key}\n"
        ),
        stderr: String::new(),
    }
}

fn grok_get_token() -> Result<String, String> {
    let auth_path = grok_auth_path();
    if !auth_path.exists() {
        return Err(format!("no active session found at {}. Run 'maw grok login' first.", auth_path.display()));
    }
    let raw = std::fs::read_to_string(&auth_path).map_err(|e| format!("failed to read auth.json at {}: {e}", auth_path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("failed to parse auth.json: {e}"))?;
    let obj = json.as_object().ok_or("invalid auth.json structure")?;
    if obj.is_empty() {
        return Err("auth.json is empty. Run 'maw grok login' first.".to_owned());
    }
    let (_, val) = obj.iter().next().unwrap();
    let token = val.get("key").and_then(serde_json::Value::as_str).ok_or("missing 'key' field in auth.json")?;
    Ok(token.to_owned())
}

async fn grok_upload_file(client: &reqwest::Client, token: &str, path: &std::path::Path) -> Result<String, String> {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("image.jpg").to_owned();
    let bytes = std::fs::read(path).map_err(|e| format!("failed to read file: {e}"))?;
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(file_name)
        .mime_str("image/jpeg").map_err(|e| format!("invalid mime type: {e}"))?;
        
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("purpose", "vision");
        
    let res = client.post("https://api.x.ai/v1/files")
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
        
    let status = res.status();
    let text = res.text().await.map_err(|e| format!("failed to read response text: {e}"))?;
    if !status.is_success() {
        return Err(format!("files upload error {status}: {text}"));
    }
    
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("failed to parse response JSON: {e}"))?;
    let id = json.pointer("/id").and_then(serde_json::Value::as_str).ok_or("missing id in response")?;
    Ok(id.to_owned())
}

async fn grok_gen_image(prompt: &str, ratio: &str) -> CliOutput {
    let token = match grok_get_token() {
        Ok(t) => t,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: {e}\n") },
    };
    
    let client = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: HTTP client build failed: {e}\n") },
    };
    
    println!("Generating image with prompt: \"{}\"...", prompt);
    let payload = serde_json::json!({
        "model": "grok-imagine-image-quality",
        "prompt": prompt,
        "aspect_ratio": ratio,
        "n": 1,
        "response_format": "url"
    });
    
    let res = client.post("https://api.x.ai/v1/images/generations")
        .bearer_auth(&token)
        .json(&payload)
        .send()
        .await;
        
    let response = match res {
        Ok(r) => r,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: request failed: {e}\n") },
    };
    
    let status = response.status();
    let body_text = match response.text().await {
        Ok(t) => t,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: failed to read response text: {e}\n") },
    };
    
    if !status.is_success() {
        return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok API error {status}: {body_text}\n") };
    }
    
    let json: serde_json::Value = match serde_json::from_str(&body_text) {
        Ok(v) => v,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: failed to parse response JSON: {e}\n") },
    };
    
    let url = match json.pointer("/data/0/url").and_then(serde_json::Value::as_str) {
        Some(u) => u,
        None => return CliOutput { code: 1, stdout: String::new(), stderr: "grok: missing url in response\n".to_owned() },
    };
    
    println!("Downloading generated image from: {}", url);
    let download_res = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: download request failed: {e}\n") },
    };
    
    let bytes = match download_res.bytes().await {
        Ok(b) => b,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: failed to read download bytes: {e}\n") },
    };
    
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
        
    let filename = format!("grok_gen_{timestamp}.jpg");
    let out_path = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")).join(&filename);
    
    if let Err(e) = std::fs::write(&out_path, &bytes) {
        return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: failed to write image file: {e}\n") };
    }
    
    CliOutput {
        code: 0,
        stdout: format!("Successfully generated image and saved to {}\n", out_path.display()),
        stderr: String::new(),
    }
}

async fn grok_gen_video(prompt: &str, img_path: Option<&str>) -> CliOutput {
    let token = match grok_get_token() {
        Ok(t) => t,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: {e}\n") },
    };
    
    let client = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: HTTP client build failed: {e}\n") },
    };
    
    let mut payload = serde_json::json!({
        "model": "grok-imagine-video-1.5",
        "prompt": prompt,
        "duration": 6
    });
    
    if let Some(path_str) = img_path {
        let path = std::path::Path::new(path_str);
        if !path.exists() {
            return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: image path does not exist: {path_str}\n") };
        }
        println!("Uploading image file {}...", path_str);
        let file_id = match grok_upload_file(&client, &token, path).await {
            Ok(id) => id,
            Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: image upload failed: {e}\n") },
        };
        println!("Uploaded file ID: {}", file_id);
        payload["image_file_id"] = serde_json::json!(file_id);
    }
    
    println!("Submitting video generation request...");
    let res = client.post("https://api.x.ai/v1/videos/generations")
        .bearer_auth(&token)
        .json(&payload)
        .send()
        .await;
        
    let response = match res {
        Ok(r) => r,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: request failed: {e}\n") },
    };
    
    let status = response.status();
    let body_text = match response.text().await {
        Ok(t) => t,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: failed to read response text: {e}\n") },
    };
    
    if !status.is_success() {
        return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok API error {status}: {body_text}\n") };
    }
    
    let json: serde_json::Value = match serde_json::from_str(&body_text) {
        Ok(v) => v,
        Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: failed to parse response JSON: {e}\n") },
    };
    
    let request_id = match json.pointer("/request_id").and_then(serde_json::Value::as_str) {
        Some(id) => id,
        None => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: missing request_id in response. Body: {body_text}\n") },
    };
    
    println!("Video generation request submitted. Request ID: {}", request_id);
    println!("Polling for status (this might take up to 2 minutes)...");
    
    let mut attempts = 0;
    let max_attempts = 60;
    loop {
        attempts += 1;
        if attempts > max_attempts {
            return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: polling timed out for request_id: {request_id}\n") };
        }
        
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        
        let poll_url = format!("https://api.x.ai/v1/videos/{}", request_id);
        let poll_res = match client.get(&poll_url).bearer_auth(&token).send().await {
            Ok(r) => r,
            Err(e) => {
                println!("grok: polling error: {e}. Retrying...");
                continue;
            }
        };
        
        let poll_status = poll_res.status();
        let poll_body = match poll_res.text().await {
            Ok(t) => t,
            Err(e) => {
                println!("grok: failed to read polling body: {e}. Retrying...");
                continue;
            }
        };
        
        if !poll_status.is_success() {
            println!("grok: polling API error {poll_status}: {poll_body}. Retrying...");
            continue;
        }
        
        let poll_json: serde_json::Value = match serde_json::from_str(&poll_body) {
            Ok(v) => v,
            Err(e) => {
                println!("grok: failed to parse polling JSON: {e}. Retrying...");
                continue;
            }
        };
        
        let job_status = poll_json.get("status").and_then(serde_json::Value::as_str).unwrap_or("pending");
        match job_status {
            "done" | "completed" | "success" => {
                let video_url = match poll_json.pointer("/video/url").or_else(|| poll_json.pointer("/url")).and_then(serde_json::Value::as_str) {
                    Some(u) => u,
                    None => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: missing video url in completed response: {poll_body}\n") },
                };
                
                println!("Downloading video from: {}", video_url);
                let download_res = match client.get(video_url).send().await {
                    Ok(r) => r,
                    Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: video download request failed: {e}\n") },
                };
                
                let bytes = match download_res.bytes().await {
                    Ok(b) => b,
                    Err(e) => return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: failed to read video download bytes: {e}\n") },
                };
                
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                    
                let filename = format!("grok_video_{timestamp}.mp4");
                let out_path = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")).join(&filename);
                
                if let Err(e) = std::fs::write(&out_path, &bytes) {
                    return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok: failed to write video file: {e}\n") };
                }
                
                return CliOutput {
                    code: 0,
                    stdout: format!("Successfully generated video and saved to {}\n", out_path.display()),
                    stderr: String::new(),
                };
            },
            "failed" | "error" => {
                let err_msg = poll_json.get("error").and_then(serde_json::Value::as_str).unwrap_or("unknown error");
                return CliOutput { code: 1, stdout: String::new(), stderr: format!("grok video generation failed: {err_msg}\n") };
            },
            _ => {
                println!("Job status: {} (attempt {}/{})", job_status, attempts, max_attempts);
            }
        }
    }
}
