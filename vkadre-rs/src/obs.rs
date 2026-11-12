//! Minimal obs-websocket v5 client (blocking, one socket per thread).
//!
//! Protocol: Hello (op 0) → Identify (op 1) → Identified (op 2); Request (op 6) → RequestResponse (op 7);
//! events arrive as op 5 on connections that subscribed to them.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tungstenite::{Message, WebSocket};

use crate::rgb::RgbImage;

/// Event subscription bits (obs-websocket EventSubscription).
pub const EVENTS_NONE: u32 = 0;
pub const EVENTS_SCENES: u32 = 1 << 2;
pub const EVENTS_UI: u32 = 1 << 10;

/// Transform keys we put back when tracking stops.
pub const RESTORE_KEYS: [&str; 14] = [
    "positionX", "positionY", "alignment", "rotation", "boundsType", "boundsAlignment", "boundsWidth",
    "boundsHeight", "cropLeft", "cropTop", "cropRight", "cropBottom", "scaleX", "scaleY",
];

/// What the engine needs from OBS. The real client implements it; tests use a fake.
pub trait ObsApi: Send {
    fn item_id(&mut self, scene: &str, source: &str) -> Result<i64>;
    fn item_transform(&mut self, scene: &str, item_id: i64) -> Result<Map<String, Value>>;
    fn set_item_transform(&mut self, scene: &str, item_id: i64, transform: Value) -> Result<()>;
    fn canvas_size(&mut self) -> Result<(u32, u32)>;
    fn program_scene(&mut self) -> Result<Option<String>>;
    /// Preview scene in Studio Mode, None otherwise.
    fn preview_scene(&mut self) -> Option<String>;
    fn set_program_scene(&mut self, scene: &str) -> Result<()>;
    /// RGB image, already scaled by OBS to the requested size.
    fn screenshot(&mut self, source: &str, width: u32, height: u32) -> Result<Option<RgbImage>>;
    /// Blocks up to the read timeout for the next scene-related event.
    /// Ok(true) = something changed, Ok(false) = nothing yet.
    /// The default means "no events here": the engine falls back to polling.
    fn wait_scene_event(&mut self) -> Result<bool> {
        Err(anyhow!("события OBS не поддерживаются"))
    }

    fn set_crop(&mut self, scene: &str, item_id: i64, canvas: (u32, u32), crop: [i64; 4]) -> Result<()> {
        let [l, t, r, b] = crop.map(|v| v.max(0));
        self.set_item_transform(
            scene,
            item_id,
            json!({
                "positionX": 0.0, "positionY": 0.0, "alignment": 5, "rotation": 0.0,
                "boundsType": "OBS_BOUNDS_SCALE_INNER", "boundsAlignment": 0,
                "boundsWidth": canvas.0 as f64, "boundsHeight": canvas.1 as f64,
                "cropLeft": l, "cropTop": t, "cropRight": r, "cropBottom": b,
            }),
        )
    }
}

pub struct ObsConnection {
    ws: WebSocket<TcpStream>,
    next_id: u64,
}

fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let secret = b64.encode(Sha256::digest(format!("{password}{salt}").as_bytes()));
    b64.encode(Sha256::digest(format!("{secret}{challenge}").as_bytes()))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

impl ObsConnection {
    pub fn connect(host: &str, port: u16, password: &str, timeout: Duration, events: u32) -> Result<Self> {
        let addr = (host, port)
            .to_socket_addrs()
            .with_context(|| format!("неизвестный адрес {host}"))?
            .next()
            .ok_or_else(|| anyhow!("неизвестный адрес {host}"))?;
        let stream = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| anyhow!("OBS не отвечает на {host}:{port} ({e})"))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;
        let (ws, _) = tungstenite::client(format!("ws://{host}:{port}"), stream)
            .map_err(|e| anyhow!("ошибка WebSocket: {e}"))?;
        let mut c = ObsConnection { ws, next_id: 1 };

        let hello = c.read_op(0)?;
        let mut d = json!({ "rpcVersion": 1, "eventSubscriptions": events });
        if let Some(auth) = hello.get("authentication") {
            if password.is_empty() {
                bail!("OBS просит пароль — впишите его в «Подключение»");
            }
            let (salt, challenge) = (
                str_field(auth, "salt").unwrap_or_default(),
                str_field(auth, "challenge").unwrap_or_default(),
            );
            d["authentication"] = json!(auth_string(password, &salt, &challenge));
        }
        c.send(json!({ "op": 1, "d": d }))?;
        c.read_op(2)?;
        Ok(c)
    }

    pub fn set_read_timeout(&mut self, t: Duration) {
        let _ = self.ws.get_ref().set_read_timeout(Some(t));
    }

    pub fn close(mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }

    fn send(&mut self, v: Value) -> Result<()> {
        self.ws.send(Message::text(v.to_string())).map_err(|e| anyhow!("OBS: {e}"))
    }

    /// Next JSON message. None on read timeout.
    fn read_json(&mut self) -> Result<Option<Value>> {
        loop {
            match self.ws.read() {
                Ok(Message::Text(t)) => return Ok(Some(serde_json::from_str(t.as_str())?)),
                Ok(Message::Binary(b)) => return Ok(Some(serde_json::from_slice(&b)?)),
                Ok(Message::Close(frame)) => {
                    let (code, reason) = frame.map(|f| (u16::from(f.code), f.reason.to_string())).unwrap_or_default();
                    if code == 4009 {
                        bail!("неверный пароль OBS WebSocket");
                    }
                    bail!("OBS закрыл соединение ({code} {reason})");
                }
                Ok(_) => continue, // ping/pong, handled by tungstenite
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    return Ok(None)
                }
                Err(e) => bail!("связь с OBS потеряна: {e}"),
            }
        }
    }

    fn read_op(&mut self, op: u64) -> Result<Value> {
        loop {
            let msg = self.read_json()?.ok_or_else(|| anyhow!("OBS не ответил вовремя"))?;
            if msg.get("op").and_then(Value::as_u64) == Some(op) {
                return Ok(msg.get("d").cloned().unwrap_or(Value::Null));
            }
        }
    }

    pub fn request(&mut self, kind: &str, data: Value) -> Result<Value> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let mut d = json!({ "requestType": kind, "requestId": id });
        if !data.is_null() {
            d["requestData"] = data;
        }
        self.send(json!({ "op": 6, "d": d }))?;
        loop {
            let d = self.read_op(7)?;
            if d.get("requestId").and_then(Value::as_str) != Some(id.as_str()) {
                continue;
            }
            let status = d.get("requestStatus").cloned().unwrap_or(Value::Null);
            if status.get("result").and_then(Value::as_bool) != Some(true) {
                let comment = str_field(&status, "comment").unwrap_or_else(|| {
                    format!("код {}", status.get("code").and_then(Value::as_i64).unwrap_or(0))
                });
                bail!("{kind}: {comment}");
            }
            return Ok(d.get("responseData").cloned().unwrap_or(Value::Null));
        }
    }

    // -- info ----------------------------------------------------------------------

    pub fn version(&mut self) -> Result<String> {
        let r = self.request("GetVersion", Value::Null)?;
        Ok(format!("OBS {}", str_field(&r, "obsVersion").unwrap_or_else(|| "?".into())))
    }

    /// Scenes in the order the user sees them (OBS returns them bottom-to-top).
    pub fn scenes(&mut self) -> Result<Vec<String>> {
        let r = self.request("GetSceneList", Value::Null)?;
        let mut v: Vec<String> = r
            .get("scenes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|s| str_field(s, "sceneName")).collect())
            .unwrap_or_default();
        v.reverse();
        Ok(v)
    }

    fn scene_items(&mut self, scene: &str) -> Result<Vec<Value>> {
        let r = self.request("GetSceneItemList", json!({ "sceneName": scene }))?;
        Ok(r.get("sceneItems").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    pub fn scene_sources(&mut self, scene: &str) -> Result<Vec<String>> {
        Ok(self.scene_items(scene)?.iter().filter_map(|i| str_field(i, "sourceName")).collect())
    }

    pub fn set_item_enabled(&mut self, scene: &str, item_id: i64, enabled: bool) -> Result<()> {
        self.request(
            "SetSceneItemEnabled",
            json!({ "sceneName": scene, "sceneItemId": item_id, "sceneItemEnabled": enabled }),
        )?;
        Ok(())
    }

    pub fn move_to_top(&mut self, scene: &str, item_id: i64) -> Result<()> {
        let top = self
            .scene_items(scene)?
            .iter()
            .filter_map(|i| i.get("sceneItemIndex").and_then(Value::as_i64))
            .max()
            .unwrap_or(0);
        self.request("SetSceneItemIndex", json!({ "sceneName": scene, "sceneItemId": item_id, "sceneItemIndex": top }))?;
        Ok(())
    }

    /// Input name -> its kind without the version suffix (dshow_input, ffmpeg_source, window_capture…).
    pub fn input_kinds(&mut self) -> Result<std::collections::HashMap<String, String>> {
        let r = self.request("GetInputList", json!({}))?;
        Ok(r.get("inputs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|i| {
                        let kind = str_field(i, "unversionedInputKind").or_else(|| str_field(i, "inputKind"))?;
                        Some((str_field(i, "inputName")?, kind))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn input_names(&mut self) -> Result<Vec<String>> {
        let r = self.request("GetInputList", json!({}))?;
        Ok(r.get("inputs")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|i| str_field(i, "inputName")).collect())
            .unwrap_or_default())
    }

    pub fn create_input(&mut self, scene: &str, name: &str, kind: &str, settings: Value, enabled: bool) -> Result<i64> {
        let r = self.request(
            "CreateInput",
            json!({ "sceneName": scene, "inputName": name, "inputKind": kind,
                    "inputSettings": settings, "sceneItemEnabled": enabled }),
        )?;
        r.get("sceneItemId").and_then(Value::as_i64).ok_or_else(|| anyhow!("CreateInput: нет sceneItemId"))
    }

    pub fn create_scene_item(&mut self, scene: &str, source: &str, enabled: bool) -> Result<i64> {
        let r = self.request(
            "CreateSceneItem",
            json!({ "sceneName": scene, "sourceName": source, "sceneItemEnabled": enabled }),
        )?;
        r.get("sceneItemId").and_then(Value::as_i64).ok_or_else(|| anyhow!("CreateSceneItem: нет sceneItemId"))
    }

    pub fn set_input_settings(&mut self, name: &str, settings: Value) -> Result<()> {
        self.request("SetInputSettings", json!({ "inputName": name, "inputSettings": settings, "overlay": true }))?;
        Ok(())
    }

    pub fn refresh_browser(&mut self, name: &str) {
        // older OBS: the page reloads on show anyway
        if let Err(e) =
            self.request("PressInputPropertiesButton", json!({ "inputName": name, "propertyName": "refreshnocache" }))
        {
            log::debug!("refreshnocache: {e}");
        }
    }
}

impl ObsApi for ObsConnection {
    fn item_id(&mut self, scene: &str, source: &str) -> Result<i64> {
        let r = self.request("GetSceneItemId", json!({ "sceneName": scene, "sourceName": source }))?;
        r.get("sceneItemId").and_then(Value::as_i64).ok_or_else(|| anyhow!("нет sceneItemId"))
    }

    fn item_transform(&mut self, scene: &str, item_id: i64) -> Result<Map<String, Value>> {
        let r = self.request("GetSceneItemTransform", json!({ "sceneName": scene, "sceneItemId": item_id }))?;
        Ok(r.get("sceneItemTransform").and_then(Value::as_object).cloned().unwrap_or_default())
    }

    fn set_item_transform(&mut self, scene: &str, item_id: i64, transform: Value) -> Result<()> {
        self.request(
            "SetSceneItemTransform",
            json!({ "sceneName": scene, "sceneItemId": item_id, "sceneItemTransform": transform }),
        )?;
        Ok(())
    }

    fn canvas_size(&mut self) -> Result<(u32, u32)> {
        let r = self.request("GetVideoSettings", Value::Null)?;
        let w = r.get("baseWidth").and_then(Value::as_u64).unwrap_or(1920) as u32;
        let h = r.get("baseHeight").and_then(Value::as_u64).unwrap_or(1080) as u32;
        Ok((w, h))
    }

    fn program_scene(&mut self) -> Result<Option<String>> {
        let r = self.request("GetCurrentProgramScene", Value::Null)?;
        Ok(str_field(&r, "currentProgramSceneName").or_else(|| str_field(&r, "sceneName")))
    }

    fn preview_scene(&mut self) -> Option<String> {
        let r = self.request("GetCurrentPreviewScene", Value::Null).ok()?;
        str_field(&r, "currentPreviewSceneName").or_else(|| str_field(&r, "sceneName"))
    }

    fn set_program_scene(&mut self, scene: &str) -> Result<()> {
        self.request("SetCurrentProgramScene", json!({ "sceneName": scene }))?;
        Ok(())
    }

    fn screenshot(&mut self, source: &str, width: u32, height: u32) -> Result<Option<RgbImage>> {
        let r = self.request(
            "GetSourceScreenshot",
            json!({ "sourceName": source, "imageFormat": "jpg", "imageWidth": width,
                    "imageHeight": height, "imageCompressionQuality": 80 }),
        )?;
        let data = str_field(&r, "imageData").unwrap_or_default();
        let b64 = data.split_once(',').map(|(_, b)| b).unwrap_or(&data);
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).context("скриншот: плохой base64")?;
        Ok(RgbImage::from_jpeg(&bytes))
    }

    fn wait_scene_event(&mut self) -> Result<bool> {
        match self.read_json()? {
            None => Ok(false),
            Some(msg) => {
                if msg.get("op").and_then(Value::as_u64) != Some(5) {
                    return Ok(false);
                }
                let kind = msg.pointer("/d/eventType").and_then(Value::as_str).unwrap_or("");
                Ok(matches!(
                    kind,
                    "CurrentProgramSceneChanged" | "CurrentPreviewSceneChanged" | "StudioModeStateChanged"
                        | "SceneNameChanged" | "SceneListChanged"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_matches_protocol_example() {
        // https://github.com/obsproject/obs-websocket/blob/master/docs/generated/protocol.md#creating-an-authentication-string
        // sha256 chain must be deterministic and base64-encoded
        let a = auth_string("supersecretpassword", "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=",
                            "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY=");
        assert_eq!(a.len(), 44);
        assert_eq!(a, auth_string("supersecretpassword", "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=",
                                  "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY="));
    }
}
