//! Read-only model discovery for the endpoint being edited in the Manager.
use reqwest::blocking::Client;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::{Error, Result};

use super::{connection_key, connection_url, managed_connection, module_setting, parse, read_connection, ApiType};

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PAGE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub api_type: ApiType,
    pub base_url: String,
    pub api_key: Option<String>,
    pub remove_key: bool,
}

#[derive(Serialize)]
pub struct Model {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct CatalogPage {
    data: Vec<ApiModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct ApiModel {
    id: String,
    display_name: Option<String>,
    name: Option<String>,
    lifecycle: Option<String>,
}

#[derive(Deserialize)]
struct OllamaList {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    model: Option<String>,
    name: Option<String>,
}

fn request_headers(api_type: ApiType, key: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    if api_type == ApiType::Anthropic {
        // Match the Claude API version and credential format used for dialogue in mod-pbc.
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    if !key.is_empty() {
        let (name, value) = if api_type == ApiType::Anthropic {
            (header::HeaderName::from_static("x-api-key"), key.to_owned())
        } else {
            (header::AUTHORIZATION, format!("Bearer {key}"))
        };
        let mut value = HeaderValue::from_str(&value)
            .map_err(|_| Error::Invalid("The API key contains unsupported characters.".into()))?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    Ok(headers)
}

pub fn list(root: &Path, request: Request) -> Result<Vec<Model>> {
    module_setting(root)?;
    let mut url = connection_url(&request.base_url)?;
    let (path, _) = managed_connection(root)?;
    let previous = read_connection(&path)?;
    let key = connection_key(
        request.api_type, url.as_str().trim_end_matches('/'), request.api_key.as_deref(),
        request.remove_key, previous.as_deref().map(parse).transpose()?,
    )?;
    let mut path = url.path_segments_mut()
        .map_err(|_| Error::Invalid("Enter a valid model API base URL.".into()))?;
    path.pop_if_empty();
    if request.api_type == ApiType::Ollama {
        path.extend(["api", "tags"]);
    } else {
        path.push("models");
    }
    drop(path);
    let headers = request_headers(request.api_type, &key)?;
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::Invalid("Could not start the model list request.".into()))?;

    let deadline = Instant::now() + TIMEOUT;
    let mut models = BTreeMap::new();
    let mut cursor = None::<String>;
    let mut seen_cursors = HashSet::new();
    loop {
        let mut page_url = url.clone();
        if let Some(cursor) = &cursor {
            page_url.query_pairs_mut().append_pair("after_id", cursor);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::Invalid("The model list request timed out.".into()));
        }
        let response = client.get(page_url).headers(headers.clone()).timeout(remaining).send()
            .map_err(|error| Error::Invalid(if error.is_timeout() {
                "The model list request timed out."
            } else {
                "Could not reach the model endpoint."
            }.into()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Invalid(format!("The provider returned HTTP {status} for its model list.")));
        }
        let mut body = Vec::new();
        response.take(MAX_PAGE_BYTES + 1).read_to_end(&mut body)
            .map_err(|_| Error::Invalid("Could not read the provider's model list.".into()))?;
        if body.len() as u64 > MAX_PAGE_BYTES {
            return Err(Error::Invalid("The provider returned an oversized model list.".into()));
        }
        let invalid_list = |_| Error::Invalid("The endpoint did not return a model list.".into());
        if request.api_type == ApiType::Ollama {
            let list: OllamaList = serde_json::from_slice(&body).map_err(invalid_list)?;
            for model in list.models {
                if let Some(id) = present(model.model).or_else(|| present(model.name)) {
                    models.insert(id.clone(), id);
                }
            }
            break;
        }
        let page: CatalogPage = serde_json::from_slice(&body).map_err(invalid_list)?;
        for model in page.data {
            if model.lifecycle.as_deref() == Some("retired") || model.id.trim().is_empty() {
                continue;
            }
            let name = present(model.display_name).or_else(|| present(model.name))
                .unwrap_or_else(|| model.id.clone());
            models.insert(model.id, name);
        }
        if !page.has_more {
            break;
        }
        cursor = page.last_id.filter(|value| !value.is_empty() && seen_cursors.insert(value.clone()));
        if cursor.is_none() {
            return Err(Error::Invalid("The provider returned an invalid model list cursor.".into()));
        }
    }

    let mut choices: Vec<_> = models.into_iter().map(|(id, name)| Model { id, name }).collect();
    choices.sort_by_cached_key(|model| model.name.to_lowercase());
    Ok(choices)
}

fn present(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::pbc::{self, ConnectionUpdate};
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    fn server(pages: Vec<(&'static str, &'static str, &'static str)>) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            pages.into_iter().map(|(status, headers, body)| {
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("Mock model endpoint did not receive its request: {error}"),
                    }
                };
                socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut request = String::new();
                let mut reader = BufReader::new(&socket);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    request.push_str(&line.to_ascii_lowercase());
                }
                write!(socket, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len()).unwrap();
                request
            }).collect()
        });
        (format!("http://127.0.0.1:{port}"), worker)
    }

    fn install() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let configs = root.path().join("Core/configs/modules");
        fs::create_dir_all(&configs).unwrap();
        fs::write(configs.join("playerbots_characters.conf.dist"),
            "[worldserver]\nPBC.Enable = 0\nPBC.DefaultConnection = ../connection.jsonc\n").unwrap();
        root
    }

    fn request(api_type: ApiType, base_url: &str, api_key: Option<&str>) -> Request {
        Request { api_type, base_url: base_url.into(), api_key: api_key.map(str::to_owned), remove_key: false }
    }

    #[test]
    fn openai_compatible_lists_current_models_and_reuses_the_matching_saved_key() {
        let root = install();
        let (endpoint, requests) = server(vec![
            ("200 OK", "", r#"{"data":[{"id":"model-a"}]}"#),
            ("200 OK", "", r#"{"data":[{"id":"model-b","name":"New Model"},{"id":"model-b","name":"New Model"}]}"#),
        ]);
        let base = format!("{endpoint}/proxy/v1");
        pbc::save(root.path(), &root.path().join("metadata"), ConnectionUpdate {
            api_type: ApiType::Openai, base_url: base.clone(), model: "saved-model".into(),
            api_key: Some("fixture-secret".into()), remove_key: false,
        }).unwrap();
        let (path, _) = managed_connection(root.path()).unwrap();
        let saved = fs::read(&path).unwrap();

        let models = list(root.path(), request(ApiType::Openai, &base, None)).unwrap();
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["model-a"]);
        let refreshed = list(root.path(), request(ApiType::Openai, &base, Some("new-draft-key"))).unwrap();
        assert_eq!(refreshed.len(), 1);
        assert_eq!((&*refreshed[0].id, &*refreshed[0].name), ("model-b", "New Model"));
        assert_eq!(fs::read(path).unwrap(), saved);
        let requests = requests.join().unwrap();
        assert!(requests.iter().all(|r| r.starts_with("get /proxy/v1/models ")));
        assert!(requests[0].contains("authorization: bearer fixture-secret\r\n"));
        assert!(requests[1].contains("authorization: bearer new-draft-key\r\n"));
    }

    #[test]
    fn ollama_lists_installed_models_without_reusing_a_key_for_another_endpoint() {
        let root = install();
        pbc::save(root.path(), &root.path().join("metadata"), ConnectionUpdate {
            api_type: ApiType::Ollama, base_url: "http://127.0.0.1:11434".into(), model: "saved-model".into(),
            api_key: Some("fixture-secret".into()), remove_key: false,
        }).unwrap();
        let (endpoint, requests) = server(vec![
            ("200 OK", "", r#"{"models":[{"model":"qwen:7b","name":"alternate"},{"name":"gemma:4b"}]}"#),
        ]);
        let models = list(root.path(), request(ApiType::Ollama, &endpoint, None)).unwrap();
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gemma:4b", "qwen:7b"]);
        let requests = requests.join().unwrap();
        assert!(requests[0].starts_with("get /api/tags "));
        assert!(!requests[0].contains("authorization:"));
    }

    #[test]
    fn anthropic_follows_cursors_and_keeps_display_names_and_exact_ids() {
        let root = install();
        let (endpoint, requests) = server(vec![
            ("200 OK", "", r#"{"data":[{"id":"claude-sonnet-test","display_name":"Claude Sonnet Test"}],"has_more":true,"last_id":"opaque/page+1"}"#),
            ("200 OK", "", r#"{"data":[{"id":"claude-haiku-test","display_name":"Claude Haiku Test"},{"id":"retired-test","lifecycle":"retired"}],"has_more":false}"#),
        ]);
        let models = list(root.path(), request(ApiType::Anthropic, &format!("{endpoint}/v1"), Some("fixture-key"))).unwrap();
        assert_eq!(models.iter().map(|m| (&*m.id, &*m.name)).collect::<Vec<_>>(), [
            ("claude-haiku-test", "Claude Haiku Test"), ("claude-sonnet-test", "Claude Sonnet Test"),
        ]);
        let requests = requests.join().unwrap();
        assert!(requests[0].starts_with("get /v1/models "));
        assert!(requests[1].starts_with("get /v1/models?after_id=opaque%2fpage%2b1 "));
        for request in requests {
            assert!(request.contains("anthropic-version: 2023-06-01\r\n"));
            assert!(request.contains("x-api-key: fixture-key\r\n"));
        }
    }

    #[test]
    fn redirects_and_http_errors_keep_the_connection_file_and_do_not_expose_the_key() {
        let root = install();
        let (endpoint, requests) = server(vec![
            ("302 Found", "Location: http://127.0.0.1:1/models\r\n", ""),
            ("401 Unauthorized", "", "fixture-secret"),
        ]);
        pbc::save(root.path(), &root.path().join("metadata"), ConnectionUpdate {
            api_type: ApiType::Openai, base_url: endpoint.clone(), model: "saved-model".into(),
            api_key: Some("fixture-secret".into()), remove_key: false,
        }).unwrap();
        let (path, _) = managed_connection(root.path()).unwrap();
        let saved = fs::read(&path).unwrap();
        let redirect = list(root.path(), request(ApiType::Openai, &endpoint, None)).err().unwrap();
        assert!(redirect.to_string().contains("302"));
        let mut no_auth = request(ApiType::Openai, &endpoint, None);
        no_auth.remove_key = true;
        let unauthorized = list(root.path(), no_auth).err().unwrap();
        assert!(unauthorized.to_string().contains("401"));
        assert!(!unauthorized.to_string().contains("fixture-secret"));
        let requests = requests.join().unwrap();
        assert!(requests[0].contains("authorization: bearer fixture-secret"));
        assert!(!requests[1].contains("authorization:"));
        assert_eq!(fs::read(path).unwrap(), saved);
    }
}
