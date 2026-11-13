//! FongMi catvod 协议数据模型（与 spider-host / composeApp 的 bean 保持字段一致）。
//! 反序列化一律宽松处理：字段缺失给默认值，类型不符尽量转字符串。

use serde::{Deserialize, Deserializer};
use serde_json::Value as Jv;
use std::collections::HashMap;

// ---------- 宽松反序列化工具 ----------

fn value_to_string(v: &Jv) -> Option<String> {
    match v {
        Jv::Null => None,
        Jv::String(s) => Some(s.clone()),
        Jv::Number(n) => Some(n.to_string()),
        Jv::Bool(b) => Some(b.to_string()),
        Jv::Object(_) | Jv::Array(_) => Some(v.to_string()),
    }
}

fn de_lenient_string<'de, D>(d: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Jv::deserialize(d)?;
    Ok(value_to_string(&v).unwrap_or_default())
}

fn de_lenient_opt_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Jv::deserialize(d)?;
    Ok(value_to_string(&v))
}

fn de_header_map<'de, D>(d: D) -> Result<Option<HashMap<String, String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Jv::deserialize(d)?;
    match v {
        Jv::Null => Ok(None),
        Jv::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(None);
            }
            match serde_json::from_str::<HashMap<String, String>>(t) {
                Ok(m) => Ok(Some(m)),
                Err(_) => Ok(None),
            }
        }
        Jv::Object(o) => {
            let mut m = HashMap::new();
            for (k, val) in o {
                m.insert(k, value_to_string(&val).unwrap_or_default());
            }
            Ok(Some(m))
        }
        _ => Ok(None),
    }
}

// ---------- 站点 ----------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Site {
    #[serde(deserialize_with = "de_lenient_string")]
    pub key: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub name: String,
    pub r#type: i32,
    #[serde(deserialize_with = "de_lenient_string")]
    pub api: String,
    pub searchable: Option<i32>,
    pub changeable: Option<i32>,
    pub play_url: Option<String>,
    pub quick_search: Option<i32>,
    pub player_type: Option<String>,
    pub categories: Vec<String>,
    #[serde(deserialize_with = "de_lenient_string")]
    pub ext: String,
    pub timeout: Option<i32>,
    pub jar: Option<String>,
    #[serde(deserialize_with = "de_header_map")]
    pub header: Option<HashMap<String, String>>,
}

impl Site {
    pub fn is_searchable(&self) -> bool {
        self.searchable == Some(1)
    }
    pub fn is_changeable(&self) -> bool {
        self.changeable == Some(1)
    }
}

/// 配置 JSON 中的顶层结构（仅解析宿主需要的字段，其余忽略）
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RawApiConfig {
    pub spider: String,
    pub sites: Vec<Site>,
}

// ---------- 分类 / 筛选 ----------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Type {
    #[serde(rename = "type_id", deserialize_with = "de_lenient_string", default)]
    pub type_id: String,
    #[serde(rename = "type_name", deserialize_with = "de_lenient_string", default)]
    pub type_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FilterValue {
    #[serde(default, deserialize_with = "de_lenient_opt_string")]
    pub n: Option<String>,
    #[serde(default, deserialize_with = "de_lenient_opt_string")]
    pub v: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Filter {
    #[serde(default, deserialize_with = "de_lenient_opt_string")]
    pub key: Option<String>,
    #[serde(default, deserialize_with = "de_lenient_opt_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "de_lenient_string")]
    pub init: String,
    #[serde(default)]
    pub value: Vec<FilterValue>,
}

// ---------- 影片 ----------

#[derive(Debug, Clone, Default, Deserialize)]
/// 注意：协议字段本身是 snake_case（vod_id/vod_name...），不能套用 camelCase 重命名
#[serde(default)]
pub struct Vod {
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_id: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_name: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub type_name: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_pic: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_remarks: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_year: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_area: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_director: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_actor: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_content: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_play_from: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_play_url: String,
    #[serde(deserialize_with = "de_lenient_string")]
    pub vod_tag: String,
}

// ---------- 播放地址 Url（兼容 字符串 / [n,v,...] 数组 / 对象 三种形态） ----------

#[derive(Debug, Clone, Default)]
pub struct UrlValue {
    pub n: String,
    pub v: String,
}

#[derive(Debug, Clone, Default)]
pub struct Url {
    pub values: Vec<UrlValue>,
    pub position: usize,
}

impl Url {
    pub fn v(&self) -> String {
        self.values
            .get(self.position)
            .map(|x| x.v.clone())
            .unwrap_or_default()
    }
    pub fn is_empty(&self) -> bool {
        self.values.iter().all(|x| x.v.is_empty())
    }
}

fn de_url<'de, D>(d: D) -> Result<Url, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Jv::deserialize(d)?;
    let mut url = Url::default();
    match v {
        Jv::Null => {}
        Jv::String(s) => url.values.push(UrlValue { n: String::new(), v: s }),
        Jv::Number(n) => url.values.push(UrlValue { n: String::new(), v: n.to_string() }),
        Jv::Array(arr) => {
            let items: Vec<Jv> = arr.into_iter().collect();
            let mut i = 0;
            while i + 1 < items.len() {
                let n = value_to_string(&items[i]).unwrap_or_default();
                let val = value_to_string(&items[i + 1]).unwrap_or_default();
                url.values.push(UrlValue { n, v: val });
                i += 2;
            }
        }
        Jv::Object(o) => {
            if let Some(Jv::Array(vals)) = o.get("values") {
                for val in vals {
                    if let Jv::Object(obj) = val {
                        url.values.push(UrlValue {
                            n: obj.get("n").and_then(value_to_string).unwrap_or_default(),
                            v: obj.get("v").and_then(value_to_string).unwrap_or_default(),
                        });
                    }
                }
            }
            if let Some(p) = o.get("position").and_then(|p| p.as_u64()) {
                url.position = p as usize;
            }
        }
        _ => {}
    }
    Ok(url)
}

// ---------- Spider 返回的 Result ----------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SpiderResult {
    #[serde(rename = "class")]
    pub types: Vec<Type>,
    pub list: Vec<Vod>,
    pub filters: HashMap<String, Vec<Filter>>,
    #[serde(deserialize_with = "de_header_map")]
    pub header: Option<HashMap<String, String>>,
    #[serde(rename = "playUrl", deserialize_with = "de_lenient_string")]
    pub play_url: String,
    pub parse: Option<i32>,
    pub jx: Option<i32>,
    #[serde(deserialize_with = "de_lenient_string")]
    pub flag: String,
    #[serde(deserialize_with = "de_url")]
    pub url: Url,
    pub pagecount: Option<i32>,
    pub code: Option<i32>,
    #[serde(deserialize_with = "de_lenient_string")]
    pub msg: String,
}

impl SpiderResult {
    pub fn from_raw(raw: &str) -> anyhow::Result<SpiderResult> {
        Ok(serde_json::from_str(raw.trim())?)
    }
}

// ---------- 选集解析（对应 Kotlin Flag.createEpisode / Vod.setVodFlags） ----------

#[derive(Debug, Clone)]
pub struct Episode {
    pub name: String,
    pub url: String,
    pub number: i32,
}

#[derive(Debug, Clone)]
pub struct Flag {
    pub name: String,
    pub episodes: Vec<Episode>,
}

/// 对应 Kotlin Utils.getDigit
pub fn get_digit(text: &str) -> i32 {
    if text.starts_with('上') || text.starts_with('下') {
        return -1;
    }
    let mut cleaned = text.to_lowercase();
    for q in ["mp4", "h264", "h265", "720p", "1080p", "2160p", "4k"] {
        cleaned = cleaned.replace(q, "");
    }
    let digits: String = cleaned.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<i32>().unwrap_or(-1)
}

/// 解析 vod_play_from / vod_play_url（$$$ 分线路，# 分集，$ 分名字和地址）
pub fn build_flags(play_from: &str, play_url: &str) -> Vec<Flag> {
    let mut out = Vec::new();
    let flags: Vec<&str> = play_from.split("$$$").collect();
    let urls: Vec<&str> = play_url.split("$$$").collect();
    for (i, f) in flags.iter().enumerate() {
        let f = f.trim();
        if f.is_empty() || i >= urls.len() {
            continue;
        }
        let mut episodes = Vec::new();
        for (idx, seg) in urls[i].split('#').enumerate() {
            if seg.is_empty() {
                continue;
            }
            let mut parts = seg.splitn(2, '$');
            let first = parts.next().unwrap_or("");
            let second = parts.next().unwrap_or("");
            let (name, url) = if second.is_empty() {
                (format!("{:02}", idx + 1), first.to_string())
            } else {
                let name = if first.trim().is_empty() {
                    format!("{:02}", idx + 1)
                } else {
                    first.trim().to_string()
                };
                (name, second.to_string())
            };
            episodes.push(Episode {
                number: get_digit(&name),
                name,
                url,
            });
        }
        episodes.sort_by_key(|e| e.number);
        out.push(Flag {
            name: f.to_string(),
            episodes,
        });
    }
    out
}
