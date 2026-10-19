// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use tauri::{Emitter, Manager};

const HOST: &str = "ark.cn-beijing.volcengineapi.com";
const REGION: &str = "cn-beijing";
const SERVICE: &str = "ark";
const VERSION: &str = "2024-01-01";
const ACTION: &str = "GetAFPUsage";

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC 接受任意长度密钥");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC 接受任意长度密钥");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

/// 候选的 .env 路径，按优先级查找：环境变量 -> 当前目录 -> 可执行文件旁 -> 源码工程目录。
fn env_file_candidates() -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("AFP_ENV_FILE") {
        if !p.trim().is_empty() {
            list.push(PathBuf::from(p));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        list.push(cwd.join(".env"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            list.push(dir.join(".env"));
            // .app 包内: Contents/MacOS/ -> 上溯三级即工程根目录
            if let Some(root) = dir.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
                list.push(root.join(".env"));
            }
        }
    }
    // 编译期工程根目录（src-tauri 的上级）
    list.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.env")));
    list
}

fn parse_env_file(path: &PathBuf) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return map,
    };
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let (k, v) = line.split_once('=').unwrap();
        let key = k.trim().to_string();
        let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
        if !key.is_empty() {
            map.insert(key, val);
        }
    }
    map
}

/// 应用配置目录（macOS 下为 ~/Library/Application Support/<identifier>/）
fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|e| format!("获取配置目录失败: {}", e))
}

/// 凭证文件路径
fn credentials_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("credentials.json"))
}

/// 偏好设置文件路径（刷新间隔等，不含任何密钥）
fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("settings.json"))
}

/* ---------- 数据源注册表 ---------- */

/// 一个数据源需要的单个凭证字段（前端据此动态渲染表单，不用为每个平台手写设置页）
struct FieldDef {
    key: &'static str,
    label: &'static str,
    secret: bool,
    placeholder: &'static str,
}

/// 一个数据源：标识、展示名、凭证字段、密钥页地址
struct SourceDef {
    id: &'static str,
    name: &'static str,
    /// 取值口径（写在前端副标题上，如「账户余额」「套餐额度」）
    subtitle: &'static str,
    help_url: &'static str,
    help_text: &'static str,
    fields: &'static [FieldDef],
    /// 凭证由程序自动获取（用户无法填写）：前端隐藏录入表单，只展示只读检测结果。
    /// 目前仅 trae 使用——它从本机 Trae CN 登录态里现场解密，不写钥匙串、不落盘。
    auto_creds: bool,
    /// 凭证表单下方的额外提示（如「建议用 RAM 子账号 AK」），空串表示不显示
    note: &'static str,
}

const VOLC_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "accessKey",
        label: "Access Key",
        secret: false,
        placeholder: "AKLT…",
    },
    FieldDef {
        key: "secretKey",
        label: "Secret Key",
        secret: true,
        placeholder: "请填写 Secret Key",
    },
];

const BEARER_FIELDS: &[FieldDef] = &[FieldDef {
    key: "apiKey",
    label: "API Key",
    secret: true,
    placeholder: "sk-…",
}];

const ALIYUN_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "accessKey",
        label: "AccessKey ID",
        secret: false,
        placeholder: "LTAI…",
    },
    FieldDef {
        key: "secretKey",
        label: "AccessKey Secret",
        secret: true,
        placeholder: "请填写 AccessKey Secret",
    },
];

/// 阿里云凭证表单下的提示：授权范围越细越安全，明确建议只读、别用主账号
const ALIYUN_NOTE: &str = "建议使用 RAM 子账号创建的 AccessKey，并只授予只读权限：在 RAM 里给这个子账号挂上「费用中心只读」「域名只读」「SSL 证书只读」三类策略即可，本工具只读余额与到期信息。请勿使用主账号 AK，也不要授予任何写权限。";

const SOURCES: &[SourceDef] = &[
    SourceDef {
        id: "trae",
        name: "Trae 积分",
        subtitle: "Code 积分额度",
        help_url: "",
        help_text: "",
        // 无用户可填字段：凭证从本机 Trae CN 登录态自动解密，见 trae_probe
        fields: &[],
        auto_creds: true,
        note: "",
    },
    SourceDef {
        id: "volcengine",
        name: "火山方舟",
        subtitle: "Agent Plan 套餐额度",
        help_url: "https://console.volcengine.com/iam/keymanage",
        help_text: "打开火山引擎密钥管理页",
        fields: VOLC_FIELDS,
        auto_creds: false,
        note: "",
    },
    SourceDef {
        id: "deepseek",
        name: "DeepSeek",
        subtitle: "账户余额",
        help_url: "https://platform.deepseek.com/api_keys",
        help_text: "打开 DeepSeek 密钥页",
        fields: BEARER_FIELDS,
        auto_creds: false,
        note: "",
    },
    SourceDef {
        id: "kimi",
        name: "Kimi 月之暗面",
        subtitle: "账户余额",
        help_url: "https://platform.moonshot.cn/console/api-keys",
        help_text: "打开 Kimi 密钥页",
        fields: BEARER_FIELDS,
        auto_creds: false,
        note: "",
    },
    SourceDef {
        id: "siliconflow",
        name: "硅基流动",
        subtitle: "账户余额",
        help_url: "https://cloud.siliconflow.cn/account/ak",
        help_text: "打开硅基流动密钥页",
        fields: BEARER_FIELDS,
        auto_creds: false,
        note: "",
    },
    SourceDef {
        id: "aliyun",
        name: "阿里云",
        subtitle: "余额 · 到期时间",
        help_url: "https://ram.console.aliyun.com/users",
        help_text: "打开 RAM 用户管理页",
        fields: ALIYUN_FIELDS,
        auto_creds: false,
        note: ALIYUN_NOTE,
    },
];

fn source_def(id: &str) -> Option<&'static SourceDef> {
    SOURCES.iter().find(|s| s.id == id)
}

/* ---------- 数据源：Trae Code 积分（自动读取本机登录态，无用户可填凭证） ---------- */

/// Trae CN 登录态文件里两个存储键：加密的登录态 + 明文的套餐信息
const TRAE_AUTH_KEY: &str = "iCubeAuthInfo://icube.cloudide";
const TRAE_SERVER_KEY: &str = "iCubeServerData://icube.cloudide";
/// 自研加密格式的 magic 头（"tc" + 版本）
const TRAE_MAGIC: [u8; 6] = [0x74, 0x63, 0x05, 0x10, 0x00, 0x00];
const TRAE_SALT_LEN: usize = 32;
const TRAE_CHECKSUM_LEN: usize = 64;
/// 密钥派生用的固定 64 字节混淆表（= byteCrypto.js 里 Sle ^ ble），hex 表示
const TRAE_PAD_HEX: &str = "4dd4c2e6b83162090e52b3c7a6733ba41cb2462b829ab58a196b39db57177524f49baf7f08e8d68d26a72e37c1a95a2f1f05a51892aef2949732b62a38aadd58";

/// Trae CN 登录态文件的候选路径（按平台取）。Electron 应用的用户目录在不同系统下位置不同：
/// - macOS：`$HOME/Library/Application Support/Trae CN/User/globalStorage/storage.json`
/// - Windows：`%APPDATA%\Trae CN\User\globalStorage\storage.json`（Windows 的「AppData\Roaming」）
fn trae_storage_candidates() -> Vec<PathBuf> {
    const REL: &str = "Trae CN/User/globalStorage/storage.json";
    let mut list: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "windows") {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.trim().is_empty() {
                list.push(PathBuf::from(appdata).join(REL));
            }
        }
        // APPDATA 拿不到时，回退到 %USERPROFILE%\AppData\Roaming
        if let Ok(profile) = std::env::var("USERPROFILE") {
            if !profile.trim().is_empty() {
                list.push(PathBuf::from(profile).join("AppData/Roaming").join(REL));
            }
        }
    } else {
        let home = std::env::var("HOME").unwrap_or_default();
        list.push(PathBuf::from(home).join("Library/Application Support").join(REL));
    }
    list
}

/// Trae CN 登录态文件路径：优先取第一个「真实存在」的候选，都不存在时回退到主候选
fn trae_storage_path() -> PathBuf {
    let candidates = trae_storage_candidates();
    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| candidates.into_iter().next().unwrap_or_default())
}

/// Trae CN 应用数据根目录：登录态在 `<app>/User/globalStorage/storage.json`，上溯三级即得
fn trae_app_dir() -> Option<PathBuf> {
    trae_storage_path()
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
}

/// 客户端设备 ID：从 `<app>/ahanet/tt_net_config.config` 里取 `device_id&#*` 后面的连续数字。
/// 签到接口要带 x-device-id，取不到就返回空串（通常仍能签到）。
fn trae_device_id() -> String {
    let Some(dir) = trae_app_dir() else {
        return String::new();
    };
    let Ok(text) = std::fs::read_to_string(dir.join("ahanet").join("tt_net_config.config")) else {
        return String::new();
    };
    const KEY: &str = "device_id&#*";
    let Some(pos) = text.find(KEY) else {
        return String::new();
    };
    text[pos + KEY.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect()
}

/// 读取并解析 Trae CN 登录态文件
fn trae_read_storage() -> Result<serde_json::Value, String> {
    let path = trae_storage_path();
    let text = std::fs::read_to_string(&path)
        .map_err(|_| "未找到 Trae CN 登录态文件，请先登录 Trae CN 桌面端".to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("Trae CN 登录态文件解析失败: {}", e))
}

/// KDF：SHA512(salt) 拼固定 64 字节混淆表 -> SHA512；取前 16 字节为 key，其后 16 字节为 iv
fn trae_derive_key_iv(salt: &[u8]) -> Result<([u8; 16], [u8; 16]), String> {
    let h1 = Sha512::digest(salt);
    let pad = hex::decode(TRAE_PAD_HEX).map_err(|e| format!("PAD 常量解析失败: {}", e))?;
    let mut buf = Vec::with_capacity(128);
    buf.extend_from_slice(&h1);
    buf.extend_from_slice(&pad);
    let h2 = Sha512::digest(&buf);
    let mut key = [0u8; 16];
    let mut iv = [0u8; 16];
    key.copy_from_slice(&h2[0..16]);
    iv.copy_from_slice(&h2[16..32]);
    Ok((key, iv))
}

/// 解密登录态：base64 -> AES-128-CBC(PKCS7) -> SHA-512 校验 -> UTF-8 明文
fn trae_decrypt_blob(b64: &str) -> Result<String, String> {
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use base64::Engine;

    let blob = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("登录态 base64 解码失败: {}", e))?;
    let body_off = TRAE_MAGIC.len() + TRAE_SALT_LEN;
    if blob.len() <= body_off || blob[..TRAE_MAGIC.len()] != TRAE_MAGIC {
        return Err("登录态存储格式未知".to_string());
    }
    let (key, iv) = trae_derive_key_iv(&blob[TRAE_MAGIC.len()..body_off])?;

    let mut buf = blob[body_off..].to_vec();
    let plain = cbc::Decryptor::<aes::Aes128>::new_from_slices(&key, &iv)
        .map_err(|e| format!("AES 初始化失败: {}", e))?
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| "AES 解密失败（长度或填充异常）".to_string())?
        .to_vec();

    if plain.len() <= TRAE_CHECKSUM_LEN {
        return Err("解密数据过短".to_string());
    }
    if Sha512::digest(&plain[TRAE_CHECKSUM_LEN..]).as_slice() != &plain[..TRAE_CHECKSUM_LEN] {
        return Err("登录态 SHA-512 校验失败".to_string());
    }
    String::from_utf8(plain[TRAE_CHECKSUM_LEN..].to_vec())
        .map_err(|e| format!("登录态明文不是合法 UTF-8: {}", e))
}

/// 解密并解析出登录态 JSON（含 token / expiredAt / refreshToken / account）
fn trae_auth_json(storage: &serde_json::Value) -> Result<serde_json::Value, String> {
    let b64 = storage
        .get(TRAE_AUTH_KEY)
        .and_then(|v| v.as_str())
        .ok_or_else(|| "登录态文件里没有登录信息（请先在 Trae CN 里登录）".to_string())?;
    serde_json::from_str(&trae_decrypt_blob(b64)?)
        .map_err(|e| format!("登录态 JSON 解析失败: {}", e))
}

/// 从明文键里取套餐身份（如 Ultra），取不到返回空串
fn trae_identity(storage: &serde_json::Value) -> String {
    storage
        .get(TRAE_SERVER_KEY)
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| {
            v.get("entitlementInfo")
                .and_then(|e| e.get("identityStr"))
                .and_then(|s| s.as_str())
                .map(|x| x.to_string())
        })
        .unwrap_or_default()
}

/// 登录态是否已过期（expiredAt 解析不出时不拦）
fn trae_expired(expired_at: &str) -> bool {
    if expired_at.trim().is_empty() {
        return false;
    }
    match chrono::DateTime::parse_from_rfc3339(expired_at.trim()) {
        Ok(dt) => dt.with_timezone(&chrono::Utc) < chrono::Utc::now(),
        Err(_) => false,
    }
}

/// 探测结果：文件路径、套餐身份、过期时间、token、不可用原因（None 表示可用）
struct TraeProbe {
    storage_path: String,
    identity: String,
    expired_at: String,
    token: Option<String>,
    /// 登录态里的用户区域（如 CN），签到接口要按它带 X-User-Region
    region: String,
    /// 客户端设备 ID，签到接口要带 x-device-id
    device_id: String,
    unavailable: Option<String>,
}

/// 现场读取本机 Trae CN 登录态并解密。不写钥匙串、不落盘，也不打印 token。
fn trae_probe() -> TraeProbe {
    let path = trae_storage_path();
    let mut probe = TraeProbe {
        storage_path: path.display().to_string(),
        identity: String::new(),
        expired_at: String::new(),
        token: None,
        region: String::new(),
        device_id: trae_device_id(),
        unavailable: None,
    };

    let storage = match trae_read_storage() {
        Ok(v) => v,
        Err(e) => {
            probe.unavailable = Some(e);
            return probe;
        }
    };
    probe.identity = trae_identity(&storage);

    let auth = match trae_auth_json(&storage) {
        Ok(v) => v,
        Err(e) => {
            probe.unavailable = Some(format!("Trae CN 登录态不可用：{}", e));
            return probe;
        }
    };

    let token = auth.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let expired = auth.get("expiredAt").and_then(|v| v.as_str()).unwrap_or("").to_string();
    probe.expired_at = expired.clone();
    probe.region = auth
        .get("userRegion")
        .and_then(|v| v.get("region"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if token.is_empty() {
        probe.unavailable =
            Some("Trae CN 登录态里没有有效的 token，请在 Trae CN 里重新登录".to_string());
        return probe;
    }
    if trae_expired(&expired) {
        probe.unavailable = Some("Trae 登录态已过期，请在 Trae CN 里重新登录".to_string());
        return probe;
    }
    probe.token = Some(token);
    probe
}

/// 取数：POST IDE 同款积分接口，返回原始响应（前端自行聚合）
async fn fetch_trae_credits(token: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .post("https://api.trae.cn/trae/api/v2/pay/ide_user_ent_usage")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Cloud-IDE-JWT {}", token))
        .json(&serde_json::json!({ "require_usage": true, "req_source": 2 }))
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {}", e))?;

    if !status.is_success() {
        return Err(format!("接口返回 HTTP {}: {}", status.as_u16(), text));
    }
    serde_json::from_str(&text).map_err(|e| format!("解析响应失败: {}", e))
}

/// 组装 trae 给前端的 data：原始响应 + 登录态元信息（套餐身份 / 过期时间 / 文件路径）+ 签到结果
fn trae_payload(
    raw: serde_json::Value,
    probe: &TraeProbe,
    checkin: Option<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "raw": raw,
        "identity": probe.identity,
        "expiredAt": probe.expired_at,
        "file": probe.storage_path,
        "checkin": checkin,
    })
}

/// 签到接口返回兼容两种格式：`{checked_in:…}` 与 `{code:0, data:{checked_in:…}}`
fn unwrap_checkin(v: &serde_json::Value) -> serde_json::Value {
    if v.get("checked_in").is_none() {
        if let Some(data) = v.get("data") {
            if data.get("checked_in").is_some() {
                return data.clone();
            }
        }
    }
    v.clone()
}

/// 签到接口的单次 POST（body 固定 `{"req_source":1}`，与积分接口的 2 不同）
async fn trae_checkin_post(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    region: &str,
    device_id: &str,
) -> Result<serde_json::Value, String> {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Cloud-IDE-JWT {}", token));
    if !device_id.is_empty() {
        req = req.header("x-device-id", device_id);
    }
    if !region.is_empty() {
        req = req.header("X-User-Region", region);
    }

    let resp = req
        .json(&serde_json::json!({ "req_source": 1 }))
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {}", e))?;
    if !status.is_success() {
        return Err(format!("接口返回 HTTP {}: {}", status.as_u16(), text));
    }
    serde_json::from_str(&text).map_err(|e| format!("解析响应失败: {}", e))
}

/// 自动签到：先查状态，今天没签过且签到开放时才领取；已签到 / 未开放就不动，避免重复领取。
/// 返回 `{ checked_in, claimed, enable, credits, message }`，前端据此展示。
async fn trae_checkin(
    token: &str,
    region: &str,
    device_id: &str,
) -> Result<serde_json::Value, String> {
    const BASE: &str = "https://api.trae.cn/trae/api/v2/ug/checkin_credits";

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let status = unwrap_checkin(
        &trae_checkin_post(&client, &format!("{}/status", BASE), token, region, device_id).await?,
    );
    let checked = status
        .get("checked_in")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let enable = status
        .get("enable")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    if checked || !enable {
        return Ok(serde_json::json!({
            "checked_in": checked,
            "claimed": false,
            "enable": enable,
            "credits": status.get("credits").cloned().unwrap_or(serde_json::Value::Null),
            "message": status.get("message").cloned().unwrap_or(serde_json::Value::Null),
        }));
    }

    let claim = trae_checkin_post(&client, &format!("{}/claim", BASE), token, region, device_id).await?;
    let ok = claim
        .get("code")
        .and_then(|v| v.as_i64())
        .map(|c| c == 0)
        .unwrap_or(false);

    // 领取成功后再查一次状态，拿到最新积分；查失败不影响「已签到」的结论
    let after = if ok {
        trae_checkin_post(&client, &format!("{}/status", BASE), token, region, device_id)
            .await
            .ok()
            .map(|v| unwrap_checkin(&v))
    } else {
        None
    };

    Ok(serde_json::json!({
        "checked_in": after
            .as_ref()
            .and_then(|v| v.get("checked_in"))
            .and_then(|v| v.as_bool())
            .unwrap_or(ok),
        "claimed": ok,
        "enable": enable,
        "credits": after
            .as_ref()
            .and_then(|v| v.get("credits"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "message": if ok {
            serde_json::Value::Null
        } else {
            claim.get("message").cloned().unwrap_or(serde_json::Value::Null)
        },
    }))
}

/// 今日签到结论的本地缓存键，存在 settings.json 里（仅本机、不含任何密钥）。
/// Trae 的签到「今天」是按用户所在时区的零点翻页的（本机时区即账号时区），
/// 所以直接用本地日期当判据即可。
const CHECKIN_CACHE_KEY: &str = "traeCheckin";
/// 领取没成功（接口抽风 / 返回非成功码）时隔多久再试一次。
/// 不跟着刷新间隔重试，免得在签到接口上打出高频请求：
/// 服务端一直说没签、而 claim 又一直失败时，若按 60 秒的刷新周期重试，
/// 通宵开着就是几百上千次请求，最容易踩风控。这里退避 30 分钟一次，
/// 当天仍有机会补上签到。
const CHECKIN_RETRY_SECS: i64 = 1800;

/// 命中今日缓存就直接复用结论，不再打 /status：已签到（或签到未开放）当天不再查，
/// 领取没成功则隔 CHECKIN_RETRY_SECS 再试一次。
fn cached_checkin(app: &tauri::AppHandle) -> Option<serde_json::Value> {
    let settings = read_settings(app);
    let cache = settings.get(CHECKIN_CACHE_KEY)?;
    let payload = cache.get("payload")?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    if cache.get("date").and_then(|v| v.as_str()) != Some(today.as_str()) {
        return None;
    }
    // 已签到、或签到没开放：结论已经确定，当天剩下的时间都不用再问服务端
    let settled = payload.get("checked_in").and_then(|v| v.as_bool()).unwrap_or(false)
        || !payload.get("enable").and_then(|v| v.as_bool()).unwrap_or(true);
    if settled {
        return Some(payload.clone());
    }
    // 没签到成功（比如领取接口失败）：半小时后再试，别每次刷新都打一遍
    let ts = cache.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
    if chrono::Local::now().timestamp() - ts < CHECKIN_RETRY_SECS {
        return Some(payload.clone());
    }
    None
}

/// 把今天的签到结论连同日期落盘，下次启动 / 下一次刷新直接复用
fn save_checkin_cache(app: &tauri::AppHandle, payload: &serde_json::Value) {
    let mut settings = read_settings(app);
    settings[CHECKIN_CACHE_KEY] = serde_json::json!({
        "date": chrono::Local::now().format("%Y-%m-%d").to_string(),
        "ts": chrono::Local::now().timestamp(),
        "payload": payload,
    });
    let _ = write_settings(app, &settings);
}

/* ---------- 凭证存取 ---------- */

/// 凭证文件（v2）：按数据源 id 索引，`active` 记录当前展示的场景
///
/// ```json
/// { "version": 2, "active": "volcengine",
///   "sources": { "volcengine": { "accessKey": "…", "secretKey": "…" },
///                "deepseek": { "apiKey": "…" } } }
/// ```
///
/// v1 是单平台平铺结构 `{ "accessKey": …, "secretKey": … }`。读取时若命中 v1，
/// 会自动迁移成上面这份（归到 sources.volcengine）并落盘一次，
/// 保证老用户升级后已存的凭证不丢（对应待办 M-09）。
fn read_store(app: &tauri::AppHandle) -> serde_json::Value {
    let Ok(path) = credentials_path(app) else {
        return serde_json::json!({ "version": 3, "sources": {} });
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return serde_json::json!({ "version": 3, "sources": {} });
    };
    let mut store: serde_json::Value =
        serde_json::from_str(&text).unwrap_or(serde_json::json!({}));

    // v1 迁移：仅当存在平铺的 accessKey / secretKey 且还没有 sources 时执行
    let legacy_ak = store
        .get("accessKey")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let legacy_sk = store
        .get("secretKey")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let migrated = store.get("sources").is_none();

    if migrated {
        let mut sources = serde_json::Map::new();
        if !legacy_ak.is_empty() && !legacy_sk.is_empty() {
            sources.insert(
                "volcengine".to_string(),
                serde_json::json!({ "accessKey": legacy_ak, "secretKey": legacy_sk }),
            );
        }
        store = serde_json::json!({
            "version": 2,
            "active": "volcengine",
            "sources": serde_json::Value::Object(sources),
        });
        let _ = write_store(app, &store);
    }
    if store.get("sources").is_none() {
        store["sources"] = serde_json::json!({});
    }
    store
}

/// 写入 JSON 文件，并把文件权限收紧为 600（仅当前用户可读）
fn write_private_json(path: &PathBuf, value: &serde_json::Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {}", e))?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("写入配置失败: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 写入凭证文件
fn write_store(app: &tauri::AppHandle, store: &serde_json::Value) -> Result<(), String> {
    write_private_json(&credentials_path(app)?, store)
}

/* ---------- 偏好设置（每场景独立刷新间隔，E-02） ---------- */

/// 默认刷新间隔（秒），也是未设置过该场景时的取值
const DEFAULT_REFRESH_SECS: u64 = 60;
const MIN_REFRESH_SECS: u64 = 5;
const MAX_REFRESH_SECS: u64 = 3600;

/// 读取偏好设置：`{ "refresh": { "<sourceId>": 60 } }`
fn read_settings(app: &tauri::AppHandle) -> serde_json::Value {
    let Ok(path) = settings_path(app) else {
        return serde_json::json!({ "refresh": {} });
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return serde_json::json!({ "refresh": {} });
    };
    let mut store: serde_json::Value =
        serde_json::from_str(&text).unwrap_or(serde_json::json!({}));
    // 文件被手工改坏（不是对象 / refresh 不是对象）时重置，避免下面索引写入时 panic
    if !store.is_object() {
        store = serde_json::json!({});
    }
    if !store.get("refresh").map(|v| v.is_object()).unwrap_or(false) {
        store["refresh"] = serde_json::json!({});
    }
    store
}

fn write_settings(app: &tauri::AppHandle, store: &serde_json::Value) -> Result<(), String> {
    write_private_json(&settings_path(app)?, store)
}

/* ---------- 凭证读写：系统钥匙串优先（E-01），不可用时退回凭证文件 ---------- */

/// 钥匙串里的服务名；账号键 = 账号 id 加 `a:` 前缀（如 a:aliyun:1），每条密码存该账号凭证的
/// JSON。加前缀是为了与 v0.2.x 的旧条目（account = 数据源 id，如 aliyun）隔离、不碰撞。
const KEYRING_SERVICE: &str = "com.tangsanlang.floating";

/// 改 identifier 之前用过的钥匙串服务名；账号迁移时会把其中的旧条目一并搬走
const LEGACY_KEYRING_SERVICE: &str = "com.usheai.afpusage";

/// 钥匙串条目里用的账号键：加前缀与旧条目（account = sourceId）隔离
fn account_key(account_id: &str) -> String {
    format!("a:{}", account_id)
}

/* ---------- 账号：把「账号」提升为一等公民（v0.3.0 多账号支持） ---------- */

/// 一个已保存账号的元信息（**不含任何密钥值**，可安全落在凭证文件里）
#[derive(Clone)]
struct AccountMeta {
    id: String,
    source_id: String,
    label: String,
}

/// 读取全部账号元信息（按保存顺序，决定 Tab 顺序）
fn read_accounts(app: &tauri::AppHandle) -> Vec<AccountMeta> {
    read_store(app)
        .get("accounts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let o = v.as_object()?;
                    let id = o.get("id")?.as_str()?.trim().to_string();
                    let source_id = o.get("sourceId")?.as_str()?.trim().to_string();
                    if id.is_empty() || source_id.is_empty() {
                        return None;
                    }
                    let label = o
                        .get("label")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(AccountMeta { id, source_id, label })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 整体重写账号元信息列表（只含 id / sourceId / label，不含密钥）
fn write_accounts(app: &tauri::AppHandle, accounts: &[AccountMeta]) -> Result<(), String> {
    let mut store = read_store(app);
    store["version"] = serde_json::json!(3);
    store["accounts"] = serde_json::json!(accounts
        .iter()
        .map(|a| serde_json::json!({ "id": a.id, "sourceId": a.source_id, "label": a.label }))
        .collect::<Vec<_>>());
    write_store(app, &store)
}

/// 生成一个该平台下尚未使用过的账号 id（形如 `aliyun:1`）
fn gen_account_id(source_id: &str, existing: &[AccountMeta]) -> String {
    let mut n = 1;
    loop {
        let cand = format!("{}:{}", source_id, n);
        if !existing.iter().any(|a| a.id == cand) {
            return cand;
        }
        n += 1;
    }
}

/// 解析账号 id →（数据源定义, 账号名）。
/// trae 这类自动凭证源没有账号概念，它的「账号 id」就是数据源 id。
fn resolve_account(
    app: &tauri::AppHandle,
    account_id: &str,
) -> Option<(&'static SourceDef, String)> {
    if let Some(def) = source_def(account_id) {
        if def.auto_creds {
            return Some((def, String::new()));
        }
    }
    let acc = read_accounts(app).into_iter().find(|a| a.id == account_id)?;
    let def = source_def(&acc.source_id)?;
    Some((def, acc.label))
}

/* ---------- 会话级解锁闸门（按账号懒加载） ---------- */

/// 本次运行内已解锁（可读钥匙串）的**账号 id** 列表。默认空，**不写盘**。
///
/// 启动阶段列表为空，因此所有读钥匙串的函数都直接返回空、绝不调用 keyring，不会弹授权框；
/// 用户**第一次**点「连接账号」（或切到某个需要凭证的账号）时，`unlock_all_accounts` 会把
/// **全部**已保存账号一次性读进内存并加入这里，此后切换任何账号都命中已解锁状态、不再重复
/// 弹框。解锁与否只对本次运行有效，下次启动仍是「首次触发才读」。
static UNLOCKED_ACCOUNTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// 该账号本次运行是否已解锁（可读钥匙串）
fn account_unlocked(id: &str) -> bool {
    UNLOCKED_ACCOUNTS
        .lock()
        .map(|v| v.iter().any(|x| x == id))
        .unwrap_or(false)
}

/// 置位 / 清除某账号的会话解锁状态（仅本次运行有效）
fn set_account_unlocked(id: &str, on: bool) {
    if let Ok(mut v) = UNLOCKED_ACCOUNTS.lock() {
        let present = v.iter().any(|x| x == id);
        if on && !present {
            v.push(id.to_string());
        } else if !on && present {
            v.retain(|x| x != id);
        }
    }
}

/// 本次运行是否有任意账号已解锁（用于判断钥匙串存储方式是否需真实探测）
fn any_account_unlocked() -> bool {
    UNLOCKED_ACCOUNTS
        .lock()
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// 钥匙串是否可用：进程内只探测一次，结果缓存下来
static KEYCHAIN_READY: Mutex<Option<bool>> = Mutex::new(None);

fn keychain_ready() -> bool {
    if let Some(ready) = KEYCHAIN_READY.lock().ok().and_then(|slot| *slot) {
        return ready;
    }
    let ready = keyring::Entry::store_status().is_ok();
    if let Ok(mut slot) = KEYCHAIN_READY.lock() {
        *slot = Some(ready);
    }
    ready
}

fn keychain_entry(account_id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &account_key(account_id))
        .map_err(|e| format!("打开系统钥匙串失败: {}", e))
}

/// 读钥匙串里某个账号的凭证；没有条目或钥匙串不可用时返回 None。
/// 是否允许访问钥匙串由调用方（read_account_creds）按会话解锁态把关。
fn read_keychain_creds(account_id: &str) -> Option<HashMap<String, String>> {
    let entry = keychain_entry(account_id).ok()?;
    let secret = entry.get_password().ok()?;
    let map: HashMap<String, String> = serde_json::from_str(&secret).ok()?;
    Some(clean_creds(map))
}

fn clean_creds(map: HashMap<String, String>) -> HashMap<String, String> {
    map.into_iter()
        .filter_map(|(k, v)| {
            let v = v.trim().to_string();
            if v.is_empty() {
                None
            } else {
                Some((k, v))
            }
        })
        .collect()
}

/// 读凭证文件里某账号的明文凭证（历史数据，或钥匙串不可用时的兜底）。
/// 明文文件里 `sources` 以「账号 id」为键；老格式（键为数据源 id）由迁移单独处理。
fn read_plaintext_creds(app: &tauri::AppHandle, account_id: &str) -> HashMap<String, String> {
    let store = read_store(app);
    let mut out = HashMap::new();
    if let Some(obj) = store
        .get("sources")
        .and_then(|s| s.get(account_id))
        .and_then(|v| v.as_object())
    {
        for (k, val) in obj {
            if let Some(s) = val.as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                out.insert(k.clone(), s.to_string());
            }
        }
    }
    out
}

/// 写入明文凭证到凭证文件（仅钥匙串不可用时使用）
fn write_plaintext_creds(
    app: &tauri::AppHandle,
    account_id: &str,
    creds: &HashMap<String, String>,
) -> Result<(), String> {
    let mut store = read_store(app);
    store["version"] = serde_json::json!(3);
    store["sources"][account_id] = serde_json::to_value(creds).map_err(|e| e.to_string())?;
    write_store(app, &store)
}

/// 从凭证文件里抹掉某个账号的明文凭证
fn remove_plaintext_creds(app: &tauri::AppHandle, account_id: &str) -> Result<(), String> {
    let mut store = read_store(app);
    let removed = store
        .get_mut("sources")
        .and_then(|s| s.as_object_mut())
        .map(|sources| sources.remove(account_id).is_some())
        .unwrap_or(false);
    if !removed {
        return Ok(());
    }
    write_store(app, &store)
}

/* ---------- 明文标记：哪些账号已保存过凭证（不含任何密钥值） ---------- */

/// 读取「已保存凭证的 id 列表」标记（存于凭证文件，仅 id、不含密钥）。
/// v0.3.0 起存的是**账号 id**；v0.2.x 存的是数据源 id，迁移期按平台兜底识别。
/// 用于在锁定态不读钥匙串时也能判断某账号「是否已保存凭证」。
fn saved_account_ids(app: &tauri::AppHandle) -> Vec<String> {
    read_store(app)
        .get("savedIds")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// 整体重写「已保存凭证的 id」标记列表
fn write_saved_ids(app: &tauri::AppHandle, ids: &[String]) -> Result<(), String> {
    let mut store = read_store(app);
    store["savedIds"] = serde_json::json!(ids);
    write_store(app, &store)
}

/// 增 / 删单个 id（幂等；只是标记，不含任何凭证值）
fn mark_saved(app: &tauri::AppHandle, id: &str, saved: bool) -> Result<(), String> {
    let mut ids = saved_account_ids(app);
    let present = ids.iter().any(|x| x == id);
    if saved && !present {
        ids.push(id.to_string());
    } else if !saved && present {
        ids.retain(|x| x != id);
    } else {
        return Ok(());
    }
    write_saved_ids(app, &ids)
}

/// 该账号是否已保存过凭证：标记里有它的账号 id、或标记里仍有它所属平台 id
/// （v0.2.x 老标记），或凭证文件里还有它的明文。
fn has_saved_creds(app: &tauri::AppHandle, account_id: &str) -> bool {
    let ids = saved_account_ids(app);
    if ids.iter().any(|x| x == account_id) {
        return true;
    }
    if let Some(acc) = read_accounts(app).into_iter().find(|a| a.id == account_id) {
        if ids.iter().any(|x| x == &acc.source_id) {
            return true;
        }
    }
    !read_plaintext_creds(app, account_id).is_empty()
}

/* ---------- 一次性迁移标记（避免每次解锁重跑） ---------- */

/// 某一次性迁移是否已执行过（存于凭证文件，参考 version 字段的写法）
fn migration_done(app: &tauri::AppHandle, key: &str) -> bool {
    read_store(app)
        .get("migrations")
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 标记某一次性迁移已执行
fn mark_migration_done(app: &tauri::AppHandle, key: &str) {
    let mut store = read_store(app);
    if !store.get("migrations").map(|v| v.is_object()).unwrap_or(false) {
        store["migrations"] = serde_json::json!({});
    }
    store["migrations"][key] = serde_json::json!(true);
    let _ = write_store(app, &store);
}

/// 读某账号凭证：钥匙串优先，取不到再退回凭证文件。
/// 该账号未解锁时直接返回空，钥匙串与明文都不读（「先连接再加载」）。
fn read_account_creds(app: &tauri::AppHandle, account_id: &str) -> HashMap<String, String> {
    if !account_unlocked(account_id) {
        return HashMap::new();
    }
    if keychain_ready() {
        if let Some(creds) = read_keychain_creds(account_id) {
            return creds;
        }
    }
    read_plaintext_creds(app, account_id)
}

/// 写某账号凭证：钥匙串可用则写钥匙串，并抹掉文件里的同一份明文
fn write_account_creds(
    app: &tauri::AppHandle,
    account_id: &str,
    creds: &HashMap<String, String>,
) -> Result<(), String> {
    if keychain_ready() {
        let entry = keychain_entry(account_id)?;
        let blob = serde_json::to_string(creds).map_err(|e| e.to_string())?;
        entry
            .set_password(&blob)
            .map_err(|e| format!("写入系统钥匙串失败: {}", e))?;
        // 同一份密钥不留两处：钥匙串写成功即清掉文件里的明文
        let _ = remove_plaintext_creds(app, account_id);
        return Ok(());
    }
    write_plaintext_creds(app, account_id, creds)
}

/// 删除某账号凭证：钥匙串条目与文件明文都清掉
fn delete_account_creds(app: &tauri::AppHandle, account_id: &str) -> Result<(), String> {
    let mut failure: Option<String> = None;
    if keychain_ready() {
        if let Ok(entry) = keychain_entry(account_id) {
            match entry.delete_credential() {
                // 本来就没有这条，也算清干净了
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => failure = Some(format!("删除钥匙串凭证失败: {}", e)),
            }
        }
    }
    let _ = remove_plaintext_creds(app, account_id);
    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/* ---------- 老数据迁移：v0.2.x 的「按平台存」搬到 v0.3.0 的「按账号存」 ---------- */

/// 读老格式的明文凭证：v0.2.x 把某平台凭证平铺在 `sources[sourceId]`。
/// 新格式的键一定带 `:`（账号 id 形如 aliyun:1），所以只有不像账号 id 时才当老数据读。
fn read_legacy_plaintext(app: &tauri::AppHandle, source_id: &str) -> HashMap<String, String> {
    if source_id.contains(':') {
        return HashMap::new();
    }
    read_plaintext_creds(app, source_id)
}

/// 读老格式的凭证：现服务名（account = sourceId）→ 旧服务名 → 明文文件。
/// **只在解锁流程里调用**（启动阶段不碰钥匙串）。
fn read_legacy_source_creds(app: &tauri::AppHandle, source_id: &str) -> HashMap<String, String> {
    if keychain_ready() {
        for svc in [KEYRING_SERVICE, LEGACY_KEYRING_SERVICE] {
            if let Ok(entry) = keyring::Entry::new(svc, source_id) {
                if let Ok(secret) = entry.get_password() {
                    if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&secret) {
                        let creds = clean_creds(map);
                        if !creds.is_empty() {
                            return creds;
                        }
                    }
                }
            }
        }
    }
    read_legacy_plaintext(app, source_id)
}

/// 清掉某平台的老位置凭证（旧服务名 + 现服务名下 account = sourceId 的条目 + 明文平铺项）。
/// 新位置写成功后才调用 —— 宁可多留一份，也不冒丢密钥的风险。
fn delete_legacy_source_creds(app: &tauri::AppHandle, source_id: &str) {
    if keychain_ready() {
        for svc in [KEYRING_SERVICE, LEGACY_KEYRING_SERVICE] {
            if let Ok(entry) = keyring::Entry::new(svc, source_id) {
                let _ = entry.delete_credential();
            }
        }
    }
    let mut store = read_store(app);
    let mut changed = false;
    if let Some(obj) = store.get_mut("sources").and_then(|s| s.as_object_mut()) {
        changed = obj.remove(source_id).is_some();
    }
    if changed {
        let _ = write_store(app, &store);
    }
}

/// 启动阶段（未解锁、**不碰钥匙串**）为老用户补登账号元信息：
/// v0.2.x 的凭证按「数据源 id」保存，`savedIds` 里存的是平台 id。这里为每个
/// 「标记里有、但还没有任何账号」的平台预建一个「默认账号」，并记下
/// 旧 sourceId → 新 accountId 的映射，等解锁时再把凭证搬进来。
/// 这样老用户升级后在锁定态也能正常看到 Tab 与「已保存 · 待连接」。
fn ensure_legacy_account_meta(app: &tauri::AppHandle) {
    if !read_accounts(app).is_empty() {
        return;
    }
    let legacy_ids = saved_account_ids(app);
    let mut accounts: Vec<AccountMeta> = Vec::new();
    let mut map = serde_json::Map::new();
    for def in SOURCES.iter().filter(|d| !d.auto_creds) {
        if !legacy_ids.iter().any(|x| x == def.id) {
            continue;
        }
        let id = gen_account_id(def.id, &accounts);
        map.insert(def.id.to_string(), serde_json::json!(id));
        accounts.push(AccountMeta {
            id,
            source_id: def.id.to_string(),
            label: "默认账号".to_string(),
        });
    }
    if accounts.is_empty() {
        return;
    }
    let mut store = read_store(app);
    store["version"] = serde_json::json!(3);
    store["accounts"] = serde_json::json!(accounts
        .iter()
        .map(|a| serde_json::json!({ "id": a.id, "sourceId": a.source_id, "label": a.label }))
        .collect::<Vec<_>>());
    store["legacyAccountMap"] = serde_json::Value::Object(map);
    let _ = write_store(app, &store);
}

/// 一次性把老凭证搬进账号模型：**只在第一次解锁时执行**（此时才能读钥匙串）。
/// 每个平台最多搬一次；已有凭证的账号不覆盖，绝不丢密钥。
fn migrate_accounts_v3(app: &tauri::AppHandle) {
    if migration_done(app, "accounts:v3") {
        return;
    }
    let legacy_map = read_store(app)
        .get("legacyAccountMap")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut accounts = read_accounts(app);

    for def in SOURCES.iter().filter(|d| !d.auto_creds) {
        // 迁移目标账号：优先启动阶段预登记的那个「默认账号」，其次该平台已有的第一个账号
        let target = legacy_map
            .get(def.id)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|id| accounts.iter().any(|a| &a.id == id))
            .or_else(|| {
                accounts
                    .iter()
                    .find(|a| a.source_id == def.id)
                    .map(|a| a.id.clone())
            });
        // 先解锁再检查，避免把「钥匙串里已有凭证」的账号误判成空而重复覆盖
        if let Some(id) = &target {
            set_account_unlocked(id, true);
            if !read_account_creds(app, id).is_empty() {
                continue;
            }
        }

        let legacy = read_legacy_source_creds(app, def.id);
        if legacy.is_empty() {
            continue;
        }
        // 目标账号存在就复用它，否则新建一个
        let acc_id = match target {
            Some(id) => id,
            None => {
                let id = gen_account_id(def.id, &accounts);
                accounts.push(AccountMeta {
                    id: id.clone(),
                    source_id: def.id.to_string(),
                    label: "默认账号".to_string(),
                });
                id
            }
        };
        if write_account_creds(app, &acc_id, &legacy).is_ok() {
            delete_legacy_source_creds(app, def.id);
            set_account_unlocked(&acc_id, true);
            let _ = mark_saved(app, &acc_id, true);
        }
    }

    write_accounts(app, &accounts).ok();

    // 旧的 active 是「数据源 id」时，改写为对应的账号 id
    let mut store = read_store(app);
    let cur = store
        .get("active")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if source_def(&cur).map(|d| !d.auto_creds).unwrap_or(false) {
        let mapped = legacy_map
            .get(&cur)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                accounts
                    .iter()
                    .find(|a| a.source_id == cur)
                    .map(|a| a.id.clone())
            });
        if let Some(id) = mapped {
            if accounts.iter().any(|a| a.id == id) {
                store["active"] = serde_json::json!(id);
            }
        }
    }
    let _ = write_store(app, &store);
    mark_migration_done(app, "accounts:v3");
}

/// 该数据源的必需字段是否都填了
fn creds_complete(def: &SourceDef, creds: &HashMap<String, String>) -> bool {
    def.fields.iter().all(|f| creds.contains_key(f.key))
}

/// 取某账号当前生效的凭证：应用内设置优先，火山方舟再兜底 .env / 环境变量。
/// 该账号未解锁时直接返回 None —— 既不读钥匙串，也跳过 .env 兜底（否则方舟会在未连接时照常出数）。
fn effective_creds(
    app: &tauri::AppHandle,
    account_id: &str,
    def: &SourceDef,
) -> Option<(HashMap<String, String>, &'static str)> {
    if !account_unlocked(account_id) {
        return None;
    }
    let saved = read_account_creds(app, account_id);
    if creds_complete(def, &saved) {
        return Some((saved, "settings"));
    }
    if def.id == "volcengine" {
        if let Some((ak, sk)) = load_env_credentials() {
            let mut m = HashMap::new();
            m.insert("accessKey".to_string(), ak);
            m.insert("secretKey".to_string(), sk);
            return Some((m, "env"));
        }
    }
    None
}

/// 读取当前账号 id：存的 id 失效时退回第一个账号；一个账号都没有时退回第一个数据源（trae）
fn active_account_id(app: &tauri::AppHandle) -> String {
    let accounts = read_accounts(app);
    let stored = read_store(app)
        .get("active")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if accounts.iter().any(|a| a.id == stored) {
        return stored;
    }
    // trae 这类自动凭证源没有账号，其 id 可被直接当作「当前」
    if let Some(def) = source_def(&stored) {
        if def.auto_creds {
            return stored;
        }
    }
    if let Some(a) = accounts.first() {
        return a.id.clone();
    }
    SOURCES[0].id.to_string()
}

/// 兜底来源：环境变量 / .env（仅方便本地开发，不参与打包发布）
fn load_env_credentials() -> Option<(String, String)> {
    let mut file_vars: HashMap<String, String> = HashMap::new();
    for path in env_file_candidates() {
        let vars = parse_env_file(&path);
        if !vars.is_empty() {
            file_vars = vars;
            break;
        }
    }

    let pick = |name: &str, alias: &str| -> Option<String> {
        std::env::var(name)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| std::env::var(alias).ok().filter(|v| !v.trim().is_empty()))
            .or_else(|| {
                file_vars
                    .get(name)
                    .or_else(|| file_vars.get(alias))
                    .filter(|v| !v.trim().is_empty())
                    .cloned()
            })
    };

    let ak = pick("VOLC_ACCESSKEY", "VOLC_AK")?;
    let sk = pick("VOLC_SECRETKEY", "VOLC_SK")?;
    Some((ak, sk))
}

/* ---------- 取数：每个平台独立实现，统一返回该平台的原始响应 ---------- */

/// 带 Bearer 鉴权的 GET，返回原始 JSON（各平台返回结构不同，由前端各自解析）
async fn http_get_json(url: &str, api_key: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {}", e))?;

    if !status.is_success() {
        return Err(format!("接口返回 HTTP {}: {}", status.as_u16(), text));
    }
    serde_json::from_str(&text).map_err(|e| format!("解析响应失败: {}", e))
}

fn require(creds: &HashMap<String, String>, key: &str, label: &str) -> Result<String, String> {
    creds
        .get(key)
        .cloned()
        .ok_or_else(|| format!("缺少 {}", label))
}

/// 按数据源 id 取数。返回该平台原始的响应体，字段结构各异。
async fn fetch_source_raw(
    source_id: &str,
    creds: &HashMap<String, String>,
) -> Result<serde_json::Value, String> {
    match source_id {
        "volcengine" => {
            let ak = require(creds, "accessKey", "Access Key")?;
            let sk = require(creds, "secretKey", "Secret Key")?;
            fetch_afp_usage(&ak, &sk).await
        }
        "deepseek" => {
            let key = require(creds, "apiKey", "API Key")?;
            http_get_json("https://api.deepseek.com/user/balance", &key).await
        }
        "kimi" => {
            let key = require(creds, "apiKey", "API Key")?;
            http_get_json("https://api.moonshot.cn/v1/users/me/balance", &key).await
        }
        "siliconflow" => {
            let key = require(creds, "apiKey", "API Key")?;
            http_get_json("https://api.siliconflow.cn/v1/user/info", &key).await
        }
        "aliyun" => {
            let ak = require(creds, "accessKey", "AccessKey ID")?;
            let sk = require(creds, "secretKey", "AccessKey Secret")?;
            fetch_aliyun_overview(&ak, &sk).await
        }
        other => Err(format!("未支持的数据源: {}", other)),
    }
}

fn build_auth_headers(ak: &str, sk: &str, x_date: &str, date_short: &str, body: &[u8]) -> String {
    let content_sha256 = sha256_hex(body);
    let signed_headers = "content-type;host;x-content-sha256;x-date";
    let canonical_headers = format!(
        "content-type:application/json\nhost:{}\nx-content-sha256:{}\nx-date:{}\n",
        HOST, content_sha256, x_date
    );
    let canonical_query = format!("Action={}&Version={}", ACTION, VERSION);
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        "POST", "/", canonical_query, canonical_headers, signed_headers, content_sha256
    );

    let credential_scope = format!("{}/{}/{}/request", date_short, REGION, SERVICE);
    let string_to_sign = format!(
        "{}\n{}\n{}\n{}",
        "HMAC-SHA256",
        x_date,
        credential_scope,
        sha256_hex(canonical_request.as_bytes())
    );

    let mut signing_key = hmac_sha256(sk.as_bytes(), date_short.as_bytes());
    signing_key = hmac_sha256(&signing_key, REGION.as_bytes());
    signing_key = hmac_sha256(&signing_key, SERVICE.as_bytes());
    signing_key = hmac_sha256(&signing_key, b"request");
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    format!(
        "HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        ak, credential_scope, signed_headers, signature
    )
}

async fn fetch_afp_usage(ak: &str, sk: &str) -> Result<serde_json::Value, String> {
    let now = chrono::Utc::now();
    let x_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_short = now.format("%Y%m%d").to_string();
    let body: Vec<u8> = b"{}".to_vec();

    let authorization = build_auth_headers(ak, sk, &x_date, &date_short, &body);
    let content_sha256 = sha256_hex(&body);

    let url = format!("https://{}/?Action={}&Version={}", HOST, ACTION, VERSION);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("X-Date", x_date)
        .header("X-Content-Sha256", content_sha256)
        .header("Authorization", authorization)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {}", e))?;

    if !status.is_success() {
        return Err(format!("接口返回 HTTP {}: {}", status.as_u16(), text));
    }

    let payload: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析响应失败: {}", e))?;

    match payload.get("Result") {
        Some(result) => Ok(result.clone()),
        None => Err(format!("返回异常: {}", text)),
    }
}

/* ---------- 数据源：阿里云（RPC 风格 1.0 版签名，HMAC-SHA1） ---------- */

// 阿里云的费用中心（BSS）与域名、证书是三套独立的 OpenAPI，各自的域名与版本号都不同，
// 到期信息也分散在三个接口里，取回后在前端合并成一张按到期时间升序的列表。
const ALIYUN_BSS_HOST: &str = "business.aliyuncs.com";
const ALIYUN_BSS_VERSION: &str = "2017-12-14";
const ALIYUN_DOMAIN_HOST: &str = "domain.aliyuncs.com";
const ALIYUN_DOMAIN_VERSION: &str = "2018-01-29";
const ALIYUN_CAS_HOST: &str = "cas.aliyuncs.com";
const ALIYUN_CAS_VERSION: &str = "2020-04-07";
/// 每页拉多少条；分页拉全时最多翻 5 页，避免异常响应把请求拖成死循环
const ALIYUN_PAGE_SIZE: u32 = 300;
const ALIYUN_MAX_PAGES: u32 = 5;

/// 阿里云签名的 URL 编码：只保留 RFC3986 的非保留字符（字母 / 数字 / - _ . ~），
/// 其余字节按大写十六进制百分号编码。注意别用 form-urlencoded，它会把空格编成 +。
fn aliyun_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 签名用的随机串：微秒时间戳 + 进程内自增序号，保证同一秒内多次请求也不重复
fn aliyun_nonce() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    format!("{}{}", micros, SEQ.fetch_add(1, Ordering::Relaxed))
}

fn json_at<'a>(v: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for key in path {
        cur = cur.get(*key)?;
    }
    Some(cur)
}

/// 调一次阿里云 RPC 接口（GET），自动拼公共参数 + 签名，返回解析后的 JSON。
/// 接口的业务错误（响应体里带 Code / Message）在这里统一转成 Err。
async fn aliyun_rpc(
    ak: &str,
    sk: &str,
    endpoint: &str,
    action: &str,
    version: &str,
    extra: &[(&str, String)],
) -> Result<serde_json::Value, String> {
    use base64::Engine;

    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut params: Vec<(String, String)> = vec![
        ("AccessKeyId".into(), ak.to_string()),
        ("Action".into(), action.to_string()),
        ("Format".into(), "JSON".to_string()),
        ("SignatureMethod".into(), "HMAC-SHA1".to_string()),
        ("SignatureNonce".into(), aliyun_nonce()),
        ("SignatureVersion".into(), "1.0".to_string()),
        ("Timestamp".into(), timestamp),
        ("Version".into(), version.to_string()),
    ];
    for (k, v) in extra {
        params.push(((*k).to_string(), v.clone()));
    }
    // 签名要求：所有参数（含公共参数）按参数名字典序排序后再拼规范查询串
    params.sort_by(|a, b| a.0.cmp(&b.0));
    let canonical = params
        .iter()
        .map(|(k, v)| format!("{}={}", aliyun_encode(k), aliyun_encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let string_to_sign = format!("GET&{}&{}", aliyun_encode("/"), aliyun_encode(&canonical));
    let key = format!("{}&", sk);
    let signature = base64::engine::general_purpose::STANDARD
        .encode(hmac_sha1(key.as_bytes(), string_to_sign.as_bytes()));

    let url = format!(
        "https://{}/?{}&Signature={}",
        endpoint,
        canonical,
        aliyun_encode(&signature)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {}", e))?;
    if !status.is_success() {
        return Err(format!("接口返回 HTTP {}: {}", status.as_u16(), text));
    }
    let payload: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析响应失败: {}", e))?;
    if let Some(code) = payload.get("Code").and_then(|v| v.as_str()) {
        let msg = payload
            .get("Message")
            .and_then(|v| v.as_str())
            .unwrap_or("未知原因");
        return Err(format!("{}（{}）", msg, code));
    }
    Ok(payload)
}

/// 分页拉全：按 size 逐页请求，直到收满 Total 或翻到页数上限。
/// list_path / total_path 是各接口响应里「列表数组」与「总数」的位置。
async fn aliyun_fetch_pages(
    ak: &str,
    sk: &str,
    endpoint: &str,
    action: &str,
    version: &str,
    extra: &[(&str, String)],
    page_param: &str,
    size_param: &str,
    list_path: &[&str],
    total_path: &[&str],
) -> Result<Vec<serde_json::Value>, String> {
    let mut all: Vec<serde_json::Value> = Vec::new();
    for page in 1..=ALIYUN_MAX_PAGES {
        let mut args: Vec<(&str, String)> = extra.to_vec();
        args.push((page_param, page.to_string()));
        args.push((size_param, ALIYUN_PAGE_SIZE.to_string()));

        let payload = aliyun_rpc(ak, sk, endpoint, action, version, &args).await?;
        let list = json_at(&payload, list_path)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let total = json_at(&payload, total_path)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let got = list.len();
        all.extend(list);
        // 收不满一页说明已经是最后一页；总数取不到（字段位置各接口不一致）时
        // 不把它当成 0 提前收工，靠翻页上限兜底
        if got < ALIYUN_PAGE_SIZE as usize {
            break;
        }
        if total > 0 && all.len() as u64 >= total {
            break;
        }
    }
    Ok(all)
}

/// Unix 毫秒时间戳 -> 统一的 ISO8601 UTC 字符串（前端用本地时区算剩余天数）
fn aliyun_iso_from_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// 解析各接口给出的到期时间，统一成 ISO8601 UTC 字符串：
/// - 实例 / 资源包：已是 `2026-11-14T16:00:00Z`
/// - 域名：优先用毫秒时间戳字段，字符串兜底（文档未标注时区，按 UTC 处理）
/// - 证书：订单型是毫秒时间戳，上传型是 `YYYY-MM-DD`
fn aliyun_norm_date(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => aliyun_iso_from_ms(n.as_i64().unwrap_or(0)),
        serde_json::Value::String(s) => {
            let raw = s.trim();
            if raw.is_empty() {
                return String::new();
            }
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
                return dt
                    .with_timezone(&chrono::Utc)
                    .format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string();
            }
            for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
                if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(raw, fmt) {
                    return ndt.format("%Y-%m-%dT%H:%M:%SZ").to_string();
                }
            }
            if let Ok(d) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
                return d
                    .and_hms_opt(0, 0, 0)
                    .map(|ndt| ndt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                    .unwrap_or_default();
            }
            String::new()
        }
        _ => String::new(),
    }
}

/// 到期时间（ISO）-> 剩余天数；解析不了返回 None
fn aliyun_days_left(iso: &str) -> Option<i64> {
    let dt = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some((dt.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_days())
}

/// 按 ProductCode 给出中文产品名；表里没有的直接回退成大写码本身（至少不会漏项）
fn aliyun_product_name(code: &str) -> String {
    let name = match code.to_ascii_lowercase().as_str() {
        "ecs" => "云服务器 ECS",
        "rds" => "云数据库 RDS",
        "polardb" => "云原生数据库 PolarDB",
        "kvstore" | "redis" => "云数据库 Redis",
        "mongodb" => "云数据库 MongoDB",
        "hbase" => "云数据库 HBase",
        "slb" => "负载均衡 SLB",
        "vpc" => "专有网络 VPC",
        "eip" => "弹性公网 IP",
        "nas" => "文件存储 NAS",
        "oss" => "对象存储 OSS",
        "ess" => "弹性伸缩 ESS",
        "cdn" | "cdns" => "CDN",
        "ddos" | "ddospro" => "DDoS 防护",
        "waf" => "Web 应用防火墙",
        "cas" => "SSL 证书",
        "dysms" | "sms" => "短信服务",
        "domain" => "域名",
        "elasticsearch" => "Elasticsearch",
        "hologres" => "Hologres",
        "adb" => "云原生数据仓库 AnalyticDB",
        "cs" | "csb" => "容器服务",
        _ => "",
    };
    if name.is_empty() {
        code.to_ascii_uppercase()
    } else {
        name.to_string()
    }
}

/// 统一的到期条目：产品名 / 资源标识 / 到期时间（ISO UTC）
fn aliyun_expiring_item(
    kind: &str,
    product: &str,
    name: &str,
    region: &str,
    end_time: &str,
) -> serde_json::Value {
    serde_json::json!({
        "kind": kind,
        "product": product,
        "name": name,
        "region": region,
        "endTime": end_time,
    })
}

fn aliyun_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn aliyun_str_num(v: &serde_json::Value, key: &str) -> String {
    match v.get(key) {
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

/// 取一张「余额 + 即将到期 + 短信余量」的总览。
/// 单个接口失败（常见于 RAM 子账号权限没给全）不整体失败：能取到的照常返回，
/// 失败的记进 warnings 交给前端提示；全都失败才返回 Err。
async fn fetch_aliyun_overview(
    ak: &str,
    sk: &str,
) -> Result<serde_json::Value, String> {
    let mut warnings: Vec<String> = Vec::new();
    let mut expiring: Vec<serde_json::Value> = Vec::new();
    let mut ok_count = 0usize;

    // 1) 账户余额（可选额度和现金余额都给出来，两个口径在控制台里含义不同）
    let mut balance = serde_json::Value::Null;
    match aliyun_rpc(
        ak,
        sk,
        ALIYUN_BSS_HOST,
        "QueryAccountBalance",
        ALIYUN_BSS_VERSION,
        &[],
    )
    .await
    {
        Ok(v) => {
            ok_count += 1;
            let data = v.get("Data").cloned().unwrap_or(serde_json::Value::Null);
            balance = serde_json::json!({
                "available": aliyun_str_num(&data, "AvailableAmount"),
                "cash": aliyun_str_num(&data, "AvailableCashAmount"),
                "currency": aliyun_str(&data, "Currency"),
            });
        }
        Err(e) => warnings.push(format!("账户余额：{}", e)),
    }

    // 2) 订阅型实例的到期时间（ECS / RDS 等一次拉全，与产品无关）
    match aliyun_fetch_pages(
        ak,
        sk,
        ALIYUN_BSS_HOST,
        "QueryAvailableInstances",
        ALIYUN_BSS_VERSION,
        &[("SubscriptionType", "Subscription".to_string())],
        "PageNum",
        "PageSize",
        &["Data", "InstanceList"],
        &["Data", "TotalCount"],
    )
    .await
    {
        Ok(list) => {
            ok_count += 1;
            for it in list {
                let end_time = aliyun_norm_date(&it["EndTime"]);
                if end_time.is_empty() {
                    continue;
                }
                let code = aliyun_str(&it, "ProductCode");
                expiring.push(aliyun_expiring_item(
                    "instance",
                    &aliyun_product_name(&code),
                    &aliyun_str(&it, "InstanceID"),
                    &aliyun_str(&it, "Region"),
                    &end_time,
                ));
            }
        }
        Err(e) => warnings.push(format!("服务器 / 数据库到期：{}", e)),
    }

    // 3) 域名到期时间（不在上面的实例列表里，要单独查）
    match aliyun_fetch_pages(
        ak,
        sk,
        ALIYUN_DOMAIN_HOST,
        "QueryDomainList",
        ALIYUN_DOMAIN_VERSION,
        &[],
        "PageNum",
        "PageSize",
        &["Data", "Domain"],
        // 域名接口的分页字段在顶层，和实例接口（在 Data 里）不一样
        &["TotalItemNum"],
    )
    .await
    {
        Ok(list) => {
            ok_count += 1;
            for it in list {
                // 文档里的字符串到期时间带时区歧义，优先用毫秒时间戳字段
                let end_time = match it.get("ExpirationDateLong") {
                    Some(v) if !v.is_null() => aliyun_norm_date(v),
                    _ => aliyun_norm_date(&it["ExpirationDate"]),
                };
                if end_time.is_empty() {
                    continue;
                }
                expiring.push(aliyun_expiring_item(
                    "domain",
                    "域名",
                    &aliyun_str(&it, "DomainName"),
                    "",
                    &end_time,
                ));
            }
        }
        Err(e) => warnings.push(format!("域名到期：{}", e)),
    }

    // 4) SSL 证书到期时间：证书订单与用户上传的证书是两个不同的 OrderType，
    //    返回的到期字段也各是一套（CertEndTime 毫秒 / EndDate 日期），分别取再合并
    for (order_type, is_order) in [("CPACK", true), ("CERT", false)] {
        let res = aliyun_fetch_pages(
            ak,
            sk,
            ALIYUN_CAS_HOST,
            "ListUserCertificateOrder",
            ALIYUN_CAS_VERSION,
            &[("OrderType", order_type.to_string())],
            "CurrentPage",
            "ShowSize",
            &["CertificateOrderList"],
            &["TotalCount"],
        )
        .await;
        match res {
            Ok(list) => {
                ok_count += 1;
                for it in list {
                    let end_time = if is_order {
                        aliyun_norm_date(&it["CertEndTime"])
                    } else {
                        aliyun_norm_date(&it["EndDate"])
                    };
                    if end_time.is_empty() {
                        continue;
                    }
                    let name = if is_order {
                        aliyun_str(&it, "Domain")
                    } else {
                        aliyun_str(&it, "CommonName")
                    };
                    expiring.push(aliyun_expiring_item(
                        "cert",
                        "SSL 证书",
                        &name,
                        "",
                        &end_time,
                    ));
                }
            }
            Err(e) => warnings.push(format!("证书到期：{}", e)),
        }
    }

    if ok_count == 0 {
        return Err(format!(
            "阿里云接口全部调用失败：{}",
            warnings.join("；")
        ));
    }

    // 只留还没到期的条目，按到期时间升序（最近的排最前，悬浮窗取第一条）
    expiring.retain(|it| {
        it.get("endTime")
            .and_then(|v| v.as_str())
            .and_then(aliyun_days_left)
            .map(|d| d >= 0)
            .unwrap_or(false)
    });
    expiring.sort_by_key(|it| {
        it.get("endTime")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default()
    });

    // 5) 短信套餐余量：官方 FAQ 明确「没有查询套餐包剩余量的 API」，这里用资源包
    //    接口试取（能不能拿到取决于账号是否被支持），拿不到就整块隐藏、不当成错误
    let mut sms = serde_json::Value::Null;
    if let Ok(payload) = aliyun_rpc(
        ak,
        sk,
        ALIYUN_BSS_HOST,
        "QueryResourcePackageInstances",
        ALIYUN_BSS_VERSION,
        &[("PageSize", "100".to_string())],
    )
    .await
    {
        let list = json_at(&payload, &["Data", "Instances", "Instance"])
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for it in list {
            let commodity = aliyun_str(&it, "CommodityCode").to_ascii_lowercase();
            let status = aliyun_str(&it, "Status");
            if !commodity.contains("sms") || (!status.is_empty() && status != "Available") {
                continue;
            }
            let remaining = aliyun_str_num(&it, "RemainingAmount");
            if remaining.is_empty() {
                continue;
            }
            sms = serde_json::json!({
                "remaining": remaining,
                "total": aliyun_str_num(&it, "TotalAmount"),
                "unit": aliyun_str(&it, "RemainingAmountUnit"),
                "endTime": aliyun_norm_date(&it["ExpiryTime"]),
            });
            break;
        }
    }

    Ok(serde_json::json!({
        "balance": balance,
        "expiring": expiring,
        "sms": sms,
        "warnings": warnings,
    }))
}

/// 平台清单：供设置弹窗渲染「平台 + 各平台账号」，并回填已保存的值。
/// **不触碰钥匙串**：已保存与否只看明文标记，凭证值仅在对应账号解锁后才回填。
#[tauri::command]
fn list_sources(app: tauri::AppHandle) -> serde_json::Value {
    // 老用户（v0.2.x，凭证按平台存）在启动阶段先把账号元信息补出来，锁定态才不会显示成「未配置」
    ensure_legacy_account_meta(&app);

    let any_unlocked = any_account_unlocked();
    let path = credentials_path(&app)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let settings = read_settings(&app);
    let interval_of = |acc_id: &str| -> u64 {
        settings
            .get("refresh")
            .and_then(|r| r.get(acc_id))
            .and_then(|v| v.as_u64())
            .map(|s| s.clamp(MIN_REFRESH_SECS, MAX_REFRESH_SECS))
            .unwrap_or(DEFAULT_REFRESH_SECS)
    };
    let accounts = read_accounts(&app);

    let sources: Vec<serde_json::Value> = SOURCES
        .iter()
        .map(|def| {
            // 平台级字段 schema（**不含值**）：新增账号时前端据此生成表单
            let schema: Vec<serde_json::Value> = def
                .fields
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "key": f.key,
                        "label": f.label,
                        "secret": f.secret,
                        "placeholder": f.placeholder,
                    })
                })
                .collect();
            let mut item = serde_json::json!({
                "id": def.id,
                "name": def.name,
                "subtitle": def.subtitle,
                "helpUrl": def.help_url,
                "helpText": def.help_text,
                "note": def.note,
                "autoCreds": def.auto_creds,
                "fields": schema,
            });

            if def.auto_creds {
                // 自动凭证源（trae）：不是真正意义上的账号，用一个固定条目承载它
                let probe = trae_probe();
                let ok = probe.unavailable.is_none();
                item["autoInfo"] = serde_json::json!({
                    "file": probe.storage_path,
                    "expiredAt": probe.expired_at,
                    "identity": probe.identity,
                    "available": ok,
                    "unavailable": probe.unavailable,
                });
                item["accounts"] = serde_json::json!([{
                    "id": def.id,
                    "label": "",
                    "configured": ok,
                    "ready": ok,
                    "unlocked": true,
                    "hasCreds": false,
                    "interval": interval_of(def.id),
                    "fields": [],
                }]);
            } else {
                let list: Vec<serde_json::Value> = accounts
                    .iter()
                    .filter(|a| a.source_id == def.id)
                    .map(|a| {
                        // 该账号本次运行是否已解锁（解锁后才读钥匙串回填真实值）
                        let unlocked = account_unlocked(&a.id);
                        let has_creds = has_saved_creds(&app, &a.id);
                        let saved = if unlocked {
                            read_account_creds(&app, &a.id)
                        } else {
                            read_plaintext_creds(&app, &a.id)
                        };
                        let fields: Vec<serde_json::Value> = def
                            .fields
                            .iter()
                            .map(|f| {
                                serde_json::json!({
                                    "key": f.key,
                                    "label": f.label,
                                    "secret": f.secret,
                                    "placeholder": f.placeholder,
                                    "value": saved.get(f.key).cloned().unwrap_or_default(),
                                })
                            })
                            .collect();
                        // 三态：未配置 / 已保存但未解锁 / 已解锁可用
                        serde_json::json!({
                            "id": a.id,
                            "label": a.label,
                            "configured": has_creds,
                            "ready": has_creds && unlocked,
                            "unlocked": unlocked,
                            "hasCreds": has_creds,
                            "interval": interval_of(&a.id),
                            "fields": fields,
                        })
                    })
                    .collect();
                item["accounts"] = serde_json::Value::Array(list);
            }
            item
        })
        .collect();

    serde_json::json!({
        "path": path,
        "active": active_account_id(&app),
        "unlocked": any_unlocked,
        // 未解锁任何账号时不探测钥匙串可用性（避免触碰 keyring），先按钥匙串假设；解锁后回填真实值
        "storage": if any_unlocked {
            if keychain_ready() { "keychain" } else { "file" }
        } else {
            "keychain"
        },
        "defaultInterval": DEFAULT_REFRESH_SECS,
        "minInterval": MIN_REFRESH_SECS,
        "maxInterval": MAX_REFRESH_SECS,
        "sources": sources,
    })
}

/// 设置某个账号的自动刷新间隔（秒），返回夹紧后的实际取值
#[tauri::command]
fn set_refresh_interval(
    app: tauri::AppHandle,
    account_id: String,
    seconds: u64,
) -> Result<u64, String> {
    resolve_account(&app, &account_id).ok_or_else(|| format!("账号不存在: {}", account_id))?;
    let secs = seconds.clamp(MIN_REFRESH_SECS, MAX_REFRESH_SECS);

    let mut settings = read_settings(&app);
    settings["refresh"][account_id.as_str()] = serde_json::json!(secs);
    write_settings(&app, &settings)?;
    Ok(secs)
}

/// 保存某平台的账号凭证：`account_id` 为空表示新增账号，否则更新已有账号。
/// 保存成功后把该账号设为当前，并返回它的账号 id。
#[tauri::command]
fn save_account_credentials(
    app: tauri::AppHandle,
    source_id: String,
    account_id: Option<String>,
    label: String,
    values: HashMap<String, String>,
) -> Result<String, String> {
    let def = source_def(&source_id).ok_or_else(|| format!("未支持的数据源: {}", source_id))?;

    if def.auto_creds {
        return Err(format!("「{}」的凭证自动读取本机登录态，无需手动填写。", def.name));
    }

    let mut clean: HashMap<String, String> = HashMap::new();
    for f in def.fields {
        let v = values.get(f.key).map(|s| s.trim().to_string()).unwrap_or_default();
        if v.is_empty() {
            return Err(format!("请填写完整的 {}", f.label));
        }
        clean.insert(f.key.to_string(), v);
    }

    let label = label.trim().to_string();
    let mut accounts = read_accounts(&app);
    let editing = account_id.filter(|s| !s.trim().is_empty());
    let acc_id = match editing {
        Some(id) => {
            let Some(acc) = accounts.iter_mut().find(|a| a.id == id && a.source_id == def.id) else {
                return Err("要修改的账号不存在".to_string());
            };
            if !label.is_empty() {
                acc.label = label;
            }
            id
        }
        None => {
            let id = gen_account_id(def.id, &accounts);
            let label = if label.is_empty() {
                "默认账号".to_string()
            } else {
                label
            };
            accounts.push(AccountMeta {
                id: id.clone(),
                source_id: def.id.to_string(),
                label,
            });
            id
        }
    };

    write_account_creds(&app, &acc_id, &clean)?;

    // 刚写入的账号本次运行即可读（无需再走一次「连接账号」）
    set_account_unlocked(&acc_id, true);

    // 账号元信息（仅 id / sourceId / label）落盘
    write_accounts(&app, &accounts)?;

    // 同步明文标记（仅 id），供锁定态判断「已保存凭证」；写钥匙串本身不弹授权框
    mark_saved(&app, &acc_id, true)?;

    // 凭证本身已进钥匙串 / 凭证文件，配置文件里只留「当前账号」这个非敏感标记
    let mut store = read_store(&app);
    store["version"] = serde_json::json!(3);
    store["active"] = serde_json::json!(acc_id.clone());
    write_store(&app, &store)?;
    Ok(acc_id)
}

/// 删除某个账号（只影响该账号，同平台其他账号与平台配置保留）
#[tauri::command]
fn delete_account(app: tauri::AppHandle, account_id: String) -> Result<(), String> {
    let mut accounts = read_accounts(&app);
    if !accounts.iter().any(|a| a.id == account_id) {
        return Err("账号不存在".to_string());
    }
    let was_active = active_account_id(&app) == account_id;

    delete_account_creds(&app, &account_id)?;
    // 该账号已无凭证，收回本次运行的解锁态
    set_account_unlocked(&account_id, false);
    mark_saved(&app, &account_id, false)?;
    accounts.retain(|a| a.id != account_id);
    write_accounts(&app, &accounts)?;

    // 删的是当前账号时，把当前切到剩下的第一个账号（没有则退回 trae）
    if was_active {
        let next = accounts
            .first()
            .map(|a| a.id.clone())
            .unwrap_or_else(|| SOURCES[0].id.to_string());
        let mut store = read_store(&app);
        store["version"] = serde_json::json!(3);
        store["active"] = serde_json::json!(next);
        write_store(&app, &store)?;
    }
    Ok(())
}

/// 连接账号：把本次会话需要的钥匙串**一次性全部**读进内存。
///
/// 启动阶段不读钥匙串、不弹授权框；用户**第一次**点「连接账号」（或切到某个需要凭证的账号）时
/// 才调用这里 —— 它随即把**所有**已保存账号的密钥一起读出来并缓存解锁态。此后切换任何账号都
/// 直接用已缓存的凭证，不再重复弹授权框（「首次触发一次、之后全局免再输」）。
/// `account_id` 仅用于校验（前端始终传当前账号），实际执行的是全量解锁。
/// 返回读到的账号：`{"unlocked": true, "accounts": ["aliyun:1", ...]}`。
#[tauri::command]
fn unlock_accounts(app: tauri::AppHandle, account_id: String) -> Result<serde_json::Value, String> {
    resolve_account(&app, &account_id).ok_or_else(|| format!("账号不存在: {}", account_id))?;
    unlock_all_accounts(&app)
}

/// 全量解锁：先跑一次性老数据迁移，再把所有已保存账号的凭证读进内存并置位解锁态。
/// 读不到凭证的账号会收回解锁态（前端仍按「待连接」展示）。
fn unlock_all_accounts(app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    // 老数据迁移必须在这里跑：它要读钥匙串里的旧条目，此时才可能弹授权框
    migrate_accounts_v3(app);

    let mut ids: Vec<String> = Vec::new();
    for acc in read_accounts(app) {
        // 先置位解锁，读取才能访问钥匙串（此时才可能弹授权框）
        set_account_unlocked(&acc.id, true);
        if read_account_creds(app, &acc.id).is_empty() {
            set_account_unlocked(&acc.id, false);
        } else {
            mark_saved(app, &acc.id, true)?;
            ids.push(acc.id);
        }
    }
    Ok(serde_json::json!({ "unlocked": true, "accounts": ids }))
}

/// 切换当前展示的账号
#[tauri::command]
fn set_active_account(app: tauri::AppHandle, account_id: String) -> Result<(), String> {
    resolve_account(&app, &account_id).ok_or_else(|| format!("账号不存在: {}", account_id))?;
    let mut store = read_store(&app);
    store["version"] = serde_json::json!(3);
    store["active"] = serde_json::json!(account_id);
    write_store(&app, &store)
}

/// 取当前（或指定）账号的数据
#[tauri::command]
async fn get_source_data(
    app: tauri::AppHandle,
    account_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let acc_id = account_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| active_account_id(&app));
    let (def, label) = resolve_account(&app, &acc_id)
        .ok_or_else(|| format!("账号不存在: {}", acc_id))?;

    // 该账号未解锁：钥匙串类账号不取数，返回可被前端识别的「待授权」结构（而非红色报错）
    if !def.auto_creds && !account_unlocked(&acc_id) {
        return Ok(serde_json::json!({
            "account": acc_id,
            "source": def.id,
            "name": def.name,
            "label": label,
            "subtitle": def.subtitle,
            "state": "locked",
        }));
    }

    let data = if def.auto_creds {
        // 现场解密本机登录态取 token；失败时把具体原因（未登录 / 已过期）回给前端
        let probe = trae_probe();
        let token = match (probe.token.clone(), probe.unavailable.clone()) {
            (Some(t), _) => t,
            (None, Some(reason)) => return Err(reason),
            (None, None) => return Err("Trae CN 登录态不可用".to_string()),
        };
        let raw = fetch_trae_credits(&token).await?;
        // 顺手做一次自动签到：今天没签过就领，签过则跳过（失败不影响积分展示）。
        // 结论按「本地日期」缓存到本机：今天已经确认过就直接复用，不再打签到接口，
        // 免得每次刷新都去问一遍 /status。
        let checkin = match cached_checkin(&app) {
            Some(cached) => Some(cached),
            None => {
                let fresh = trae_checkin(&token, &probe.region, &probe.device_id)
                    .await
                    .ok();
                if let Some(v) = fresh.as_ref() {
                    save_checkin_cache(&app, v);
                }
                fresh
            }
        };
        trae_payload(raw, &probe, checkin)
    } else {
        let (creds, _) = effective_creds(&app, &acc_id, def).ok_or_else(|| {
            format!("尚未配置「{}」的凭证，请点击右上角「设置」填写。", def.name)
        })?;
        fetch_source_raw(def.id, &creds).await?
    };

    Ok(serde_json::json!({
        "account": acc_id,
        "source": def.id,
        "name": def.name,
        "label": label,
        "subtitle": def.subtitle,
        "data": data,
    }))
}

/// 用尚未保存的凭证试调一次接口，用于「测试连接」
#[tauri::command]
async fn test_source(
    source_id: String,
    values: HashMap<String, String>,
) -> Result<serde_json::Value, String> {
    let def = source_def(&source_id).ok_or_else(|| format!("未支持的数据源: {}", source_id))?;

    if def.auto_creds {
        // 无用户字段：直接现场解密登录态试调
        let probe = trae_probe();
        let token = match (probe.token.clone(), probe.unavailable.clone()) {
            (Some(t), _) => t,
            (None, Some(reason)) => return Err(reason),
            (None, None) => return Err("Trae CN 登录态不可用".to_string()),
        };
        let raw = fetch_trae_credits(&token).await?;
        return Ok(serde_json::json!({ "source": def.id, "data": trae_payload(raw, &probe, None) }));
    }

    let mut creds: HashMap<String, String> = HashMap::new();
    for f in def.fields {
        match values
            .get(f.key)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            Some(v) => {
                creds.insert(f.key.to_string(), v);
            }
            None => return Err(format!("请先填写 {}", f.label)),
        }
    }

    let data = fetch_source_raw(def.id, &creds).await?;
    Ok(serde_json::json!({ "source": def.id, "data": data }))
}

/// 用系统默认浏览器打开链接（设置弹窗里的「密钥管理页」）
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("仅支持 http(s) 链接".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("打开链接失败: {}", e))?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .spawn()
            .map_err(|e| format!("打开链接失败: {}", e))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("打开链接失败: {}", e))?;
    }
    Ok(())
}

/// Windows 专有：在「定住」悬浮模式下拦截最小化，避免 Win+D（显示桌面）/
/// Win+M（最小化所有窗口）把置顶悬浮窗一起隐藏。
///
/// 直接声明 user32 的少量 API，不引入 windows crate，避免与其版本 / feature 组合
/// 产生类型或链接上的差异。
#[cfg(target_os = "windows")]
mod float_guard {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

    const GWLP_WNDPROC: i32 = -4;
    const WM_SIZE: u32 = 0x0005;
    const WM_SYSCOMMAND: u32 = 0x0112;
    const SC_MINIMIZE: usize = 0xF020;
    const SIZE_MINIMIZED: usize = 1;
    const SW_RESTORE: i32 = 9;

    type Hwnd = *mut c_void;
    type Lresult = isize;
    type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> Lresult;

    #[link(name = "user32")]
    extern "system" {
        fn SetWindowLongPtrW(hwnd: Hwnd, nindex: i32, dwnewlong: isize) -> isize;
        fn CallWindowProcW(
            lp_prev_wnd_proc: Option<WndProc>,
            hwnd: Hwnd,
            msg: u32,
            wparam: usize,
            lparam: isize,
        ) -> Lresult;
        fn ShowWindow(hwnd: Hwnd, ncmdshow: i32) -> i32;
    }

    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static PREV: AtomicIsize = AtomicIsize::new(0);

    /// 开关：仅在「定住」悬浮模式下拦截最小化。
    pub fn set_active(on: bool) {
        ACTIVE.store(on, Ordering::SeqCst);
    }

    /// 子类化主窗口过程。GWLP_WNDPROC 只能在创建窗口的线程上设置，
    /// 因此必须从 Tauri 的 setup 钩子（主线程）调用。
    pub fn install(hwnd: Hwnd) {
        unsafe {
            let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, guard_proc as usize as isize);
            if prev != 0 {
                PREV.store(prev, Ordering::SeqCst);
            }
        }
    }

    unsafe extern "system" fn guard_proc(
        hwnd: Hwnd,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> Lresult {
        if ACTIVE.load(Ordering::SeqCst) {
            // 吞掉最小化命令，窗口保持显示
            if msg == WM_SYSCOMMAND && (wparam & 0xFFF0) == SC_MINIMIZE {
                return 0;
            }
            // 兜底：若仍被最小化，立即还原
            if msg == WM_SIZE && wparam == SIZE_MINIMIZED {
                ShowWindow(hwnd, SW_RESTORE);
                return 0;
            }
        }
        let prev = PREV.load(Ordering::SeqCst);
        CallWindowProcW(
            std::mem::transmute::<isize, Option<WndProc>>(prev),
            hwnd,
            msg,
            wparam,
            lparam,
        )
    }
}

// 普通窗口尺寸
const NORMAL_W: f64 = 420.0;
const NORMAL_H: f64 = 500.0;
const NORMAL_MIN_W: f64 = 380.0;
// 高度下限要低于最矮的 tab 内容（如 Trae 积分只有一张卡，贴合高度约 250），
// 否则「按内容自适应高度」会被 min_size 钳制住缩不下去。
const NORMAL_MIN_H: f64 = 240.0;
// 悬浮窗（定住）尺寸
// 宽度按内容自适应：前端量出卡片「贴合内容」时的宽度后调 set_float_width 收缩窗口，
// 上限 COMPACT_W（容纳最长文案，如 Trae「Ultra · 10.0 天（11-14 重置）」、
// 火山「近 5 小时剩余 + MM-DD HH:MM 重置」；264 会截断 Trae 带天数的副标题），
// 短内容平台（如 Trae 的积分）就不用再在右侧空出一大片。
// 高度以内容里最高的元素（52px 的圆环）为基准：卡片竖向留白 = COMPACT_H - 52，
// 定 61 后上下各约 4.5px（70 是各 9px，用户要求再减半）。
const COMPACT_W: f64 = 320.0;
/// Windows 下字体回退到微软雅黑，同样的文案比 macOS 宽一截：实测 320 会把 Trae 那个
/// 带天数的副标题（…天（MM-DD 重置））截成省略号。Windows 上限放宽，macOS 文案本就
/// 放得下、保持 320 不动。
const COMPACT_W_WINDOWS: f64 = 384.0;

/// 当前平台允许的悬浮窗最大宽度
fn compact_max_w() -> f64 {
    if cfg!(windows) {
        COMPACT_W_WINDOWS
    } else {
        COMPACT_W
    }
}

const COMPACT_H: f64 = 61.0;
const COMPACT_MIN_W: f64 = 180.0;
const COMPACT_MIN_H: f64 = 56.0;

// 悬浮窗放大 / 缩小过渡动画：总时长与每帧间隔
const ANIM_MS: u64 = 180;
const ANIM_FRAME_MS: u64 = 8;

/// 悬浮窗（定住态）上次摆放的物理坐标与「是否定住」都存在 settings.json 里，
/// 跨进程保留：下次启动直接回到退出时那个悬浮窗，而不是回到主界面。
/// 仅内存态没法满足「退出软件后下次回到悬浮窗」的需求，所以落盘（不含任何密钥）。
static PINNED: AtomicBool = AtomicBool::new(false);

/// 是否正在退出：退出流程里若再收到窗口 CloseRequested 就放行，
/// 避免「关窗转定住」的拦截把退出卡住
static EXITING: AtomicBool = AtomicBool::new(false);

/// 取消定住的过渡是否还在进行：线程里要先等标题栏恢复（最长约 600ms）再播 180ms 动画，
/// 这期间窗口仍是悬浮态的窄尺寸，若放任高度贴合插进来，它会「保持当前宽度」把窄宽度固化下来，
/// 覆盖掉动画的落点。贴合请求在这期间直接丢弃，动画结束后前端会再补一次兜底贴合。
static UNPIN_ANIMATING: AtomicBool = AtomicBool::new(false);

/// 写入「定住」状态与悬浮窗坐标（`pos` 为 None 时保留原有的坐标）
fn persist_pin_state(app: &tauri::AppHandle, pinned: bool, pos: Option<(i32, i32)>) {
    let mut settings = read_settings(app);
    settings["pinned"] = serde_json::json!(pinned);
    if let Some((x, y)) = pos {
        settings["floatPos"] = serde_json::json!({ "x": x, "y": y });
    }
    let _ = write_settings(app, &settings);
}

/// 上次是否处于「定住」态
fn saved_pinned(app: &tauri::AppHandle) -> bool {
    read_settings(app)
        .get("pinned")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 上次悬浮窗摆放的物理坐标
fn saved_float_pos(app: &tauri::AppHandle) -> Option<(i32, i32)> {
    let settings = read_settings(app);
    let p = settings.get("floatPos")?;
    Some((p.get("x")?.as_i64()? as i32, p.get("y")?.as_i64()? as i32))
}

/// 记录悬浮窗当前位置（在窗口仍为悬浮尺寸时调用）。
fn save_float_pos(window: &tauri::WebviewWindow) {
    let pos = window.outer_position().ok().map(|p| (p.x, p.y));
    persist_pin_state(window.app_handle(), false, pos);
}

/// 把目标外框左上角约束到当前显示器工作区（已扣除菜单栏 / Dock / 任务栏）。
/// `size` 为窗口「目标」外框尺寸（含标题栏等 chrome），返回修正后的坐标。
fn clamped_pos(window: &tauri::WebviewWindow, pos: (i32, i32), size: (u32, u32)) -> (i32, i32) {
    let Ok(Some(monitor)) = window.current_monitor() else {
        return pos;
    };

    let area = monitor.work_area();
    let (ax, ay) = (area.position.x, area.position.y);
    let (aw, ah) = (area.size.width as i32, area.size.height as i32);
    let (sw, sh) = (size.0 as i32, size.1 as i32);

    // 窗口比工作区还大时，上限取工作区左上角，避免 min > max 触发 clamp 断言。
    let x = pos.0.clamp(ax, (ax + aw - sw).max(ax));
    let y = pos.1.clamp(ay, (ay + ah - sh).max(ay));
    (x, y)
}

/// 逻辑尺寸 -> 物理尺寸（按当前显示器缩放比）。
fn physical_size(window: &tauri::WebviewWindow, w: f64, h: f64) -> (u32, u32) {
    let scale = window.scale_factor().unwrap_or(1.0);
    ((w * scale).round() as u32, (h * scale).round() as u32)
}

/// 窗口外框与内尺寸的差值（标题栏 / 边框占用的像素），与窗口大小无关。
fn chrome_delta(window: &tauri::WebviewWindow) -> (i32, i32) {
    match (window.outer_size(), window.inner_size()) {
        (Ok(o), Ok(i)) => (
            (o.width as i32 - i.width as i32).max(0),
            (o.height as i32 - i.height as i32).max(0),
        ),
        _ => (0, 0),
    }
}

/// 缓动函数：先快后慢，收尾平稳。
fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// 插值并取整。
fn lerp_i32(from: i32, to: i32, t: f64) -> i32 {
    (from as f64 + (to as f64 - from as f64) * t).round() as i32
}

/// 让窗口从「当前坐标 + 当前尺寸」平滑过渡到目标矩形。
///
/// - `to_pos` 为目标外框左上角（物理像素，与 `set_position` 语义一致）
/// - `to_size` 为目标内尺寸（物理像素，与 `set_size` 语义一致）
///
/// 起点取自窗口实时状态，终点即目标矩形本身，最后一帧精确落到目标值，
/// 因此动画结束不会出现「再跳一下」。
fn animate_rect(window: &tauri::WebviewWindow, to_pos: (i32, i32), to_size: (u32, u32)) {
    let (Ok(from_pos), Ok(from_size)) = (window.outer_position(), window.inner_size()) else {
        // 读不到起点时退化为直接落到目标，至少保证位置正确
        let _ = window.set_size(tauri::PhysicalSize::new(to_size.0, to_size.1));
        let _ = window.set_position(tauri::PhysicalPosition::new(to_pos.0, to_pos.1));
        return;
    };

    let (fx, fy) = (from_pos.x, from_pos.y);
    let (fw, fh) = (from_size.width as i32, from_size.height as i32);

    let steps = (ANIM_MS / ANIM_FRAME_MS).max(1);
    for step in 1..=steps {
        let t = ease_out_cubic(step as f64 / steps as f64);
        let x = lerp_i32(fx, to_pos.0, t);
        let y = lerp_i32(fy, to_pos.1, t);
        let w = lerp_i32(fw, to_size.0 as i32, t).max(1) as u32;
        let h = lerp_i32(fh, to_size.1 as i32, t).max(1) as u32;
        let _ = window.set_size(tauri::PhysicalSize::new(w, h));
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
        std::thread::sleep(std::time::Duration::from_millis(ANIM_FRAME_MS));
    }

    // 精确落到目标，消除逐帧取整的累计误差
    let _ = window.set_size(tauri::PhysicalSize::new(to_size.0, to_size.1));
    let _ = window.set_position(tauri::PhysicalPosition::new(to_pos.0, to_pos.1));
}

/// 置为「定住」形态：小尺寸下限、不可缩放、无边框、置顶、无阴影。
/// 启动恢复与手动点「定住」共用，启动时不走动画。
fn apply_pinned_style(window: &tauri::WebviewWindow) -> Result<(), String> {
    window
        .set_min_size(Some(tauri::LogicalSize::new(COMPACT_MIN_W, COMPACT_MIN_H)))
        .map_err(|e| e.to_string())?;
    window.set_resizable(false).map_err(|e| e.to_string())?;
    window.set_always_on_top(true).map_err(|e| e.to_string())?;
    window.set_decorations(false).map_err(|e| e.to_string())?;
    // 关闭 macOS 原生窗口阴影：透明窗口下它会在悬浮卡片四周的透明区域里投出一圈灰色阴影
    window.set_shadow(false).map_err(|e| e.to_string())?;
    Ok(())
}

/// 计算「定住」后的目标矩形：高度固定为悬浮高度，宽度取前端量到的贴合宽度
/// （`None` 则用平台上限 compact_max_w），位置优先还原上次摆放处，否则沿用当前坐标。
fn pinned_target(window: &tauri::WebviewWindow, width: Option<f64>) -> ((i32, i32), (u32, u32)) {
    let w = width.unwrap_or(compact_max_w()).clamp(COMPACT_MIN_W, compact_max_w());
    let target_size = physical_size(window, w, COMPACT_H);
    let cur = window.outer_position().map(|p| (p.x, p.y)).unwrap_or((0, 0));
    let pos = clamped_pos(window, saved_float_pos(window.app_handle()).unwrap_or(cur), target_size);
    (pos, target_size)
}

/// 切换「定住」悬浮窗模式：缩小窗口、无边框、置顶。
#[tauri::command]
async fn set_pinned(
    window: tauri::WebviewWindow,
    on: bool,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<(), String> {
    if on {
        apply_pinned_style(&window)?;

        // 提前算好最终落点（宽度用前端量出的贴合宽度，动画一次到位、动画后不再跳一下）
        let (target_pos, target_size) = pinned_target(&window, width);

        // 先让前端切到悬浮布局，再动画，保证收缩过程里展示的就是悬浮卡片
        let _ = window.emit("pin-state", true);
        animate_rect(&window, target_pos, target_size);

        // 记住「已定住」与当前坐标：中途直接退出软件也能回到这里
        PINNED.store(true, Ordering::Relaxed);
        persist_pin_state(window.app_handle(), true, Some(target_pos));

        // 定住期间拦截 Win+D / Win+M，避免悬浮窗被一并隐藏
        #[cfg(target_os = "windows")]
        float_guard::set_active(true);
    } else {
        // 在窗口仍为悬浮尺寸时记录当前位置，供下次定住时还原
        save_float_pos(&window);
        PINNED.store(false, Ordering::Relaxed);
        // 退出悬浮态时收起提示窗，避免它孤零零挂在主界面上方
        if let Some(tip) = window.app_handle().get_webview_window("tip") {
            let _ = tip.hide();
        }
        #[cfg(target_os = "windows")]
        float_guard::set_active(false);
        window.set_always_on_top(false).map_err(|e| e.to_string())?;
        window.set_shadow(true).map_err(|e| e.to_string())?;

        // macOS 上 set_decorations(true) 是通过 GCD 异步重设 styleMask 的，而
        // set_title_bar_style(Overlay) 里的 FullSizeContentView 是同步读取当前 styleMask，
        // 若紧接着调用会被随后生效的异步 mask 覆盖，导致窗口顶部残留 28px 原生标题栏空带、
        // 红绿灯失效。因此放到独立线程：先恢复 decorations，等异步 mask 生效后再套用 Overlay。
        //
        // 注意：动画期间保持悬浮态的最小尺寸（COMPACT_MIN），否则从 236x100 放大到 420x500
        // 的中间帧都小于 380x240，会被系统 min_size 钳制导致动画失效；等动画结束再放开。
        let w = window.clone();
        UNPIN_ANIMATING.store(true, Ordering::Relaxed);
        std::thread::spawn(move || {
            let _ = w.set_decorations(true);
            // set_decorations 的 styleMask 是异步生效的：轮询「外框 - 内尺寸」的差值，
            // 一旦出现标题栏占用就说明 mask 已落地，立即套用 Overlay，把中间那一下
            // 原生标题栏（红绿灯）闪现的时间压到最短。
            for _ in 0..60 {
                if chrome_delta(&w).1 > 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            #[cfg(target_os = "macos")]
            let _ = w.set_title_bar_style(tauri::TitleBarStyle::Overlay);
            // 等 Overlay 生效（标题栏占用归零）后再测量目标矩形
            for _ in 0..60 {
                if chrome_delta(&w).1 == 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }

            // 目标矩形：普通宽度 + 前端量出的内容贴合高度（没给则回落 NORMAL_H），
            // 取消定住的动画直接落到贴合高度，落地后不再二次调高
            // （此时标题栏已恢复，chrome 差值准确）
            let (cw, ch) = chrome_delta(&w);
            let scale = w.scale_factor().unwrap_or(1.0);
            let cap = w
                .current_monitor()
                .ok()
                .flatten()
                .map(|m| m.work_area().size.height as f64 / scale - ch as f64 / scale - 12.0)
                .unwrap_or(NORMAL_H);
            let h = height
                .unwrap_or(NORMAL_H)
                .clamp(NORMAL_MIN_H, cap.max(NORMAL_MIN_H));
            let inner = physical_size(&w, NORMAL_W, h);
            let outer = (
                (inner.0 as i32 + cw).max(0) as u32,
                (inner.1 as i32 + ch).max(0) as u32,
            );
            let cur = w.outer_position().map(|p| (p.x, p.y)).unwrap_or((0, 0));
            let target_pos = clamped_pos(&w, cur, outer);

            let _ = w.emit("pin-state", false);
            animate_rect(&w, target_pos, inner);

            // 动画结束后再放开最小尺寸与可缩放
            let _ = w.set_min_size(Some(tauri::LogicalSize::new(NORMAL_MIN_W, NORMAL_MIN_H)));
            let _ = w.set_resizable(true);
            UNPIN_ANIMATING.store(false, Ordering::Relaxed);
        });
    }
    Ok(())
}

/// 主窗口高度按当前 tab 内容自适应：前端量出内容「贴合高度」后回调。
/// 窗口顶边不动、底边以 ~180ms 动画收放（复用 animate_rect）；定住态不走这里
/// （悬浮卡有自己的尺寸逻辑）。高度夹在 [NORMAL_MIN_H, 当前显示器工作区] 之间，
/// 内容比屏幕还高时停在封顶值、内容区自己滚动。
#[tauri::command]
async fn fit_window_height(
    window: tauri::WebviewWindow,
    height: f64,
    animate: bool,
) -> Result<(), String> {
    if PINNED.load(Ordering::Relaxed) {
        return Ok(());
    }
    // 取消定住的过渡期：窗口还是悬浮态窄尺寸，此刻贴合会把窄宽度固化，直接丢弃
    if UNPIN_ANIMATING.load(Ordering::Relaxed) {
        return Ok(());
    }
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let (_, ch) = chrome_delta(&window);
    let cap = window
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| m.work_area().size.height as f64 / scale - ch as f64 / scale - 12.0)
        .unwrap_or(NORMAL_H);
    let target = height.clamp(NORMAL_MIN_H, cap.max(NORMAL_MIN_H));

    let cur = window.inner_size().map_err(|e| e.to_string())?;
    let cur_h = cur.height as f64 / scale;
    if (cur_h - target).abs() < 1.0 {
        return Ok(());
    }
    let (pw, ph) = physical_size(&window, cur.width as f64 / scale, target);
    if !animate {
        return window
            .set_size(tauri::PhysicalSize::new(pw, ph))
            .map_err(|e| e.to_string());
    }
    // 顶边保持不动：起点终点同坐标，animate_rect 只插值高度
    let pos = window
        .outer_position()
        .map(|p| (p.x, p.y))
        .map_err(|e| e.to_string())?;
    animate_rect(&window, pos, (pw, ph));
    Ok(())
}

/// 悬浮卡按内容自适应宽度：前端量出卡片「贴合内容」时的宽度后回调。
/// 只在定住态生效（普通窗口的尺寸由用户自己拖），并把宽度夹在 [COMPACT_MIN_W, COMPACT_W] 之间。
#[tauri::command]
fn set_float_width(window: tauri::WebviewWindow, width: f64) -> Result<(), String> {
    if !PINNED.load(Ordering::Relaxed) {
        return Ok(());
    }
    let w = width.clamp(COMPACT_MIN_W, compact_max_w());
    let (pw, ph) = physical_size(&window, w, COMPACT_H);
    window
        .set_size(tauri::PhysicalSize::new(pw, ph))
        .map_err(|e| e.to_string())?;

    // 提示窗宽度跟卡片走：数字位数变化会改卡片宽度，提示窗开着时也得跟着改，
    // 否则会和卡片左右错开
    if let Some(tip) = window.app_handle().get_webview_window("tip") {
        if tip.is_visible().unwrap_or(false) {
            let scale = window.scale_factor().map_err(|e| e.to_string())?;
            let tip_h = (TIP_LAST_H.load(Ordering::Relaxed) as f64 * scale).round() as u32;
            let _ = place_tip(&window, &tip, tip_h);
        }
    }
    Ok(())
}

/* ---------- 悬浮卡片右键菜单（系统菜单） ---------- */

/// 悬浮卡片右键菜单的两个菜单项 id
const CARD_MENU_UNPIN: &str = "card-unpin";
const CARD_MENU_QUIT: &str = "card-quit";

/// 弹出悬浮卡片的右键菜单。菜单由系统绘制，因此可以超出悬浮窗这个极小的窗口边界
/// （在网页里自绘菜单会被窗口裁掉），贴屏幕边缘时的翻转 / 回收也由系统负责，
/// 不会出现被屏幕底边裁掉的情况。
/// 「退出悬浮窗」发回前端走 set_pinned，与卡片上的 × 同一条路径；
/// 「退出软件」直接退出——退出前会由 run 回调把定住状态与坐标落盘。
#[tauri::command]
async fn show_card_menu(window: tauri::WebviewWindow) -> Result<(), String> {
    let menu = tauri::menu::MenuBuilder::new(&window)
        .text(CARD_MENU_UNPIN, "退出悬浮窗")
        .text(CARD_MENU_QUIT, "退出软件")
        .build()
        .map_err(|e| e.to_string())?;
    window.popup_menu(&menu).map_err(|e| e.to_string())
}

/// 启动时前端来问「上次是不是定住态」：窗口在 setup 里已按定住形态摆好，
/// 前端据此在窗口还隐藏时就把布局切成悬浮卡片，再调 show_window，避免主界面闪一下。
#[tauri::command]
fn startup_state(app: tauri::AppHandle) -> serde_json::Value {
    // version 取编译进二进制的包版本（与 tauri.conf.json / 安装包的版本号同步升），
    // 前端把它显示在设置面板底部，方便确认当前跑的是哪个构建
    serde_json::json!({
        "pinned": saved_pinned(&app),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// 显示主窗口。窗口以 visible:false 创建，等前端把布局（主界面 / 悬浮卡片）切好再显示。
#[tauri::command]
fn show_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())
}

/* ---------- 悬浮卡片悬停提示窗 ---------- */

/// 提示窗尺寸（逻辑像素）与它和卡片之间的间距。
/// 宽度不写死：直接取悬浮卡窗口当前的宽度，提示窗才能和卡片左右对齐、宽窄一致
/// （见 place_tip）。高度按内容自适应：各平台提示行数不同，固定 186 会让行数少的
/// （比如 Trae 只有 4 行）下方空出一大块。TIP_H 只作为首帧兜底高度与上限。
const TIP_H: f64 = 186.0;
const TIP_MIN_H: f64 = 64.0;
const TIP_GAP: f64 = 6.0;

/// 上一次量到的提示内容高度（逻辑像素）。下次显示先用它开窗，
/// 避免先按 186 开、再被前端量到的高度缩一下的可见跳动。
static TIP_LAST_H: AtomicU32 = AtomicU32::new(186);

/// 提示窗「光标离开就收起」看门狗的世代号。
///
/// 网页层只靠 `mouseleave` 收提示窗，但窗口被程序改尺寸、或指针快速移出这个很小的
/// 卡片窗口时，WebKit 有可能根本不派发 mouseleave，提示窗就一直挂在屏幕上。
/// 这里改用系统光标的真实坐标兜底：与网页事件是否送达无关。每次显示 / 隐藏都自增
/// 世代号，让上一轮看门狗作废，避免它误收一个已经重新显示的提示窗。
static TIP_WATCH: AtomicU64 = AtomicU64::new(0);

/// 判定光标是否仍在悬浮卡（主窗口）内：Some(true) 在内、Some(false) 已离开、
/// None 表示窗口或光标信息取不到（调用方据此停掉看门狗，避免空转）。
fn cursor_inside_main(app: &tauri::AppHandle) -> Option<bool> {
    let main = app.get_webview_window("main")?;
    let cursor = app.cursor_position().ok()?;
    let pos = main.outer_position().ok()?;
    let size = main.outer_size().ok()?;
    // 留 4px 余量，避免指针贴着边缘时来回抖
    let m = 4.0;
    let inside = cursor.x >= pos.x as f64 - m
        && cursor.x <= (pos.x + size.width as i32) as f64 + m
        && cursor.y >= pos.y as f64 - m
        && cursor.y <= (pos.y + size.height as i32) as f64 + m;
    Some(inside)
}

/// 看门狗用：提示窗已经不可见就没有继续看的必要，直接返回 None。
fn tip_cursor_inside(app: &tauri::AppHandle) -> Option<bool> {
    let tip = app.get_webview_window("tip")?;
    if !tip.is_visible().ok()? {
        return None;
    }
    cursor_inside_main(app)
}

/// 挂一个看门狗：每 200ms 在主线程上核对一次光标是否还在卡片窗口内，离开即收起提示窗。
/// 窗口 / 光标查询都要在主线程做，所以用 run_on_main_thread 派发。
fn spawn_tip_watchdog(app: tauri::AppHandle, gen: u64) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if TIP_WATCH.load(Ordering::SeqCst) != gen {
            return;
        }
        let app_main = app.clone();
        let app_check = app.clone();
        let _ = app_main.run_on_main_thread(move || {
            if TIP_WATCH.load(Ordering::SeqCst) != gen {
                return;
            }
            match tip_cursor_inside(&app_check) {
                Some(true) => {}
                Some(false) => {
                    // 光标已离开卡片窗口：作废本轮看门狗并收起提示窗
                    TIP_WATCH.fetch_add(1, Ordering::SeqCst);
                    if let Some(tip) = app_check.get_webview_window("tip") {
                        let _ = tip.hide();
                    }
                }
                None => {
                    // 信息取不到（窗口已关 / 已隐藏）：停掉看门狗，别空转
                    TIP_WATCH.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
    });
}

/// 把提示窗摆到卡片正上方（上方放不下就挪到下方）。
/// 宽度直接取悬浮卡窗口的宽度——卡片本来就是按内容自适应的，
/// 提示窗跟着它走，两者左右对齐、宽窄一致。
fn place_tip(
    main: &tauri::WebviewWindow,
    tip: &tauri::WebviewWindow,
    height: u32,
) -> Result<(), String> {
    let scale = main.scale_factor().map_err(|e| e.to_string())?;
    let pos = main.outer_position().map_err(|e| e.to_string())?;
    let size = main.outer_size().map_err(|e| e.to_string())?;

    let gap = (TIP_GAP * scale).round() as i32;

    let top = main
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| m.work_area().position.y)
        .unwrap_or(0);

    let mut y = pos.y - height as i32 - gap;
    if y < top {
        y = pos.y + size.height as i32 + gap;
    }
    y = y.max(top);

    // 尺寸 / 位置没有变化就不动窗口：对已显示的透明窗口重复 set_size + set_position
    // 会让它在 macOS 上闪一下，而取数刷新时这两者常常是不变的。
    let want = tauri::PhysicalSize::new(size.width, height);
    let resize = tip.outer_size().map(|s| s != want).unwrap_or(true);
    if resize {
        let _ = tip.set_size(want);
    }
    let want_pos = tauri::PhysicalPosition::new(pos.x, y);
    let mov = tip.outer_position().map(|p| p != want_pos).unwrap_or(true);
    if mov {
        let _ = tip.set_position(want_pos);
    }
    Ok(())
}

/// 显示悬浮卡片的悬停提示窗。
///
/// 单独开一个窗口而不是在卡片窗口里画：卡片窗口只有 264x100，提示要挂在卡片上方、
/// 内容比窗口还高，在原窗口里画一定会被裁掉。提示窗在 setup 里设了
/// `ignore_cursor_events`，不接收任何鼠标事件，所以不会挡住它后面（桌面或其他应用）的点击。
/// 内容由前端算好传进来（套餐 / 剩余 / 已用 / 预计可用天数等），这里只负责定位与显隐。
#[tauri::command]
fn show_float_tip(app: tauri::AppHandle, payload: serde_json::Value) -> Result<(), String> {
    let main = app.get_webview_window("main").ok_or("找不到主窗口")?;
    let tip = app.get_webview_window("tip").ok_or("找不到提示窗口")?;

    // 提示窗已经显示着：这次只是取数完成后把里面的数字刷新一下（main.js 的 refresh
    // 结束时会再调一次）。此时绝不能再 place_tip + show：对已可见的窗口重新 set_size
    // 并再 order front 一次，在 macOS 的透明窗口上会明显闪一下，而且会多挂一个看门狗
    // 把正在跑的那个顶掉。只把新内容推过去，窗口尺寸变化交给 tooltip.js 量的
    // set_tip_height、宽度变化交给 set_float_width，它们各自会重摆。
    if tip.is_visible().unwrap_or(false) {
        let _ = tip.emit("tip-data", payload);
        return Ok(());
    }

    // 窗口已经不可见，但前端可能仍以为它开着：网页层丢过一次 mouseleave，看门狗按真实
    // 光标位置把窗口收掉了，而 main.js 的 tipShown 还停在 true。于是它每次取数刷新都会
    // 再调一次 show——若此刻光标已经不在卡片上，就会在离卡片很远的地方「闪一下又没了」。
    // 光标确实不在卡片内时直接不显示；位置取不到则不拦，保持原来的显示行为。
    if cursor_inside_main(&app) == Some(false) {
        return Ok(());
    }

    let scale = main.scale_factor().map_err(|e| e.to_string())?;
    let tip_h = (TIP_LAST_H.load(Ordering::Relaxed) as f64 * scale).round() as u32;

    place_tip(&main, &tip, tip_h)?;
    // 先把内容推过去再显示，避免中间闪一帧空白
    let _ = tip.emit("tip-data", payload);
    tip.show().map_err(|e| e.to_string())?;
    // 显示成功后挂上「光标离开卡片就自动收起」看门狗（兜底网页层可能丢失的 mouseleave）
    let gen = TIP_WATCH.fetch_add(1, Ordering::SeqCst) + 1;
    spawn_tip_watchdog(app.clone(), gen);
    Ok(())
}

/// 提示窗按内容自适应高度：提示窗前端量出内容真实高度后回调（逻辑像素）。
/// 宽度跟卡片走、位置按同一规则重摆，都交给 place_tip；这里只更新高度。
#[tauri::command]
fn set_tip_height(app: tauri::AppHandle, height: f64) -> Result<(), String> {
    let main = app.get_webview_window("main").ok_or("找不到主窗口")?;
    let tip = app.get_webview_window("tip").ok_or("找不到提示窗口")?;

    let h = height.clamp(TIP_MIN_H, TIP_H);
    TIP_LAST_H.store(h as u32, Ordering::Relaxed);

    let scale = main.scale_factor().map_err(|e| e.to_string())?;
    let tip_h = (h * scale).round() as u32;
    place_tip(&main, &tip, tip_h)
}

/// 隐藏悬停提示窗（鼠标离开卡片、开始拖拽、退出悬浮态时调用）
#[tauri::command]
fn hide_float_tip(app: tauri::AppHandle) -> Result<(), String> {
    // 作废当前看门狗，避免它下一轮又去收一个刚重新显示的提示窗
    TIP_WATCH.fetch_add(1, Ordering::SeqCst);
    if let Some(tip) = app.get_webview_window("tip") {
        tip.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // 启动阶段零钥匙串访问：两次一次性迁移（明文→钥匙串、遗留服务名迁移）
            // 已挪到「连接账号」按需解锁流程里执行，见 unlock_accounts。

            // Windows：子类化主窗口过程，为「定住」悬浮窗保留防最小化的能力
            #[cfg(target_os = "windows")]
            {
                if let Some(win) = app.get_webview_window("main") {
                    if let Ok(hwnd) = win.hwnd() {
                        float_guard::install(hwnd.0);
                    }
                }
            }

            // 悬停提示窗：纯展示，不接收鼠标事件（不挡住后面的点击）、不抢焦点
            if let Some(tip) = app.get_webview_window("tip") {
                let _ = tip.set_ignore_cursor_events(true);
                let _ = tip.set_focusable(false);
            }

            let Some(win) = app.get_webview_window("main") else {
                return Ok(());
            };

            // 上次是定住态：窗口还隐藏着，先把悬浮形态与坐标摆好（不走动画，否则会有一次无意义的缩放）
            if saved_pinned(app.handle()) {
                PINNED.store(true, Ordering::Relaxed);
                if let Err(e) = apply_pinned_style(&win) {
                    eprintln!("恢复悬浮窗失败：{e}");
                }
                let (pos, size) = pinned_target(&win, None);
                let _ = win.set_size(tauri::PhysicalSize::new(size.0, size.1));
                let _ = win.set_position(tauri::PhysicalPosition::new(pos.0, pos.1));
                #[cfg(target_os = "windows")]
                float_guard::set_active(true);
            }

            // 兜底：前端若没能调 show_window（脚本出错），3 秒后强制显示，
            // 免得出现「一个窗口都没有、只能去活动监视器杀进程」的死局。
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                if let Some(w) = handle.get_webview_window("main") {
                    if !w.is_visible().unwrap_or(true) {
                        let _ = w.show();
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // 主窗口的关闭按钮不退出软件：拦下关闭并通知前端走「定住」同一条路径，
            // 效果与界面里的「定住」按钮完全一致（收缩成悬浮卡、置顶、无边框）
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main"
                    && !PINNED.load(Ordering::Relaxed)
                    && !EXITING.load(Ordering::Relaxed)
                {
                    api.prevent_close();
                    let _ = window.emit("close-to-pin", ());
                }
            }
        })
        // 右键菜单项点击：「退出悬浮窗」与卡片上的 × 走同一条路径，
        // 「退出软件」直接退出（退出前由 run 回调落盘定住状态与坐标）
        .on_menu_event(|app, event| match event.id().0.as_str() {
            CARD_MENU_UNPIN => {
                let _ = app.emit("menu-unpin", ());
            }
            CARD_MENU_QUIT => app.exit(0),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            list_sources,
            save_account_credentials,
            delete_account,
            set_active_account,
            set_refresh_interval,
            get_source_data,
            test_source,
            unlock_accounts,
            open_url,
            set_pinned,
            set_float_width,
            fit_window_height,
            set_tip_height,
            show_card_menu,
            startup_state,
            show_window,
            show_float_tip,
            hide_float_tip
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 退出前落盘：定住态下把「当前」坐标存下来，下次启动才能回到退出时那一刻的位置
            if let tauri::RunEvent::ExitRequested { .. } = event {
                EXITING.store(true, Ordering::Relaxed);
                if PINNED.load(Ordering::Relaxed) {
                    let pos = app
                        .get_webview_window("main")
                        .and_then(|w| w.outer_position().ok())
                        .map(|p| (p.x, p.y));
                    persist_pin_state(app, true, pos);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 自校验：读取本机 Trae CN 登录态，断言解密结果的 SHA-512 校验通过且能解析出 token。
    /// 文件不存在时跳过（不失败）。断言中不输出 token 内容。
    #[test]
    fn trae_decrypt_self_check() {
        let path = trae_storage_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            println!("跳过：未找到 Trae CN 登录态文件（{}）", path.display());
            return;
        };
        let storage: serde_json::Value =
            serde_json::from_str(&text).expect("登录态文件应为合法 JSON");

        let auth = trae_auth_json(&storage).expect("解密 + SHA-512 校验应通过");
        let token = auth.get("token").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!token.is_empty(), "解密结果应能解析出非空 token");

        println!(
            "Trae 登录态解密自校验通过（SHA-512 校验 OK，token 已解析但未输出）；套餐身份：{}",
            trae_identity(&storage)
        );
    }

    /// 默认状态下没有任何账号被解锁：读钥匙串的路径都被闸门挡住，
    /// 因此启动过程不会触碰钥匙串、不会弹出 macOS 授权框。
    #[test]
    fn locked_reads_skip_keychain() {
        assert!(!any_account_unlocked(), "会话解锁集合默认应为空");
        assert!(!account_unlocked("aliyun:1"), "未解锁的账号应返回 false");
    }
}
