use crate::backend::MAX_REQUEST;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Serialize;
use serde_json::{Map, Number, Value};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct GraphDocument { pub path: String, pub graph: Value }
#[derive(Serialize)]
pub struct SavedGraph { pub path: String }

// serde_json::Value 默认静默覆盖重复key；文件编辑必须明确拒绝歧义输入。
struct StrictValue { depth: usize }
impl<'de> DeserializeSeed<'de> for StrictValue {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if self.depth > 64 { return Err(de::Error::custom("JSON嵌套不能超过64层")); }
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for StrictValue {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result { formatter.write_str("合法且无重复key的JSON") }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> { Ok(Value::Null) }
    fn visit_none<E: de::Error>(self) -> Result<Value, E> { Ok(Value::Null) }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> { Ok(Value::Bool(value)) }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> { Ok(Value::Number(value.into())) }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value).map(Value::Number).ok_or_else(|| de::Error::custom("JSON数值必须有限"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> { Ok(Value::String(value.into())) }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> { Ok(Value::String(value)) }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(StrictValue { depth: self.depth + 1 })? { values.push(value); }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) { return Err(de::Error::custom(format!("重复JSON key：{key}"))); }
            let value = map.next_value_seed(StrictValue { depth: self.depth + 1 })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

pub(crate) fn strict_json(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_REQUEST { return Err("Graph JSON超过4MiB".into()); }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictValue { depth: 0 }.deserialize(&mut deserializer).map_err(|e| format!("JSON无效：{e}"))?;
    deserializer.end().map_err(|e| format!("JSON尾部无效：{e}"))?;
    if !value.is_object() { return Err("Graph文件必须包含一个JSON对象".into()); }
    Ok(value)
}

fn json_path(workspace: &Path, requested: &str, existing: bool) -> Result<PathBuf, String> {
    if requested.is_empty() || requested.contains('\0') { return Err("文件路径为空或包含NUL".into()); }
    let requested = PathBuf::from(requested);
    if !requested.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("json")) {
        return Err("只允许.json文件".into());
    }
    let target = if requested.is_absolute() { requested } else { workspace.join(requested) };
    let resolved = if existing {
        std::fs::canonicalize(&target).map_err(|e| format!("无法定位Graph文件：{e}"))?
    } else {
        let parent = target.parent().ok_or("文件缺少父目录")?;
        let parent = std::fs::canonicalize(parent).map_err(|e| format!("输出父目录不存在：{e}"))?;
        parent.join(target.file_name().ok_or("文件名无效")?)
    };
    // workspace在连接时已canonical，不能重新解析被替换的根并扩大授权范围。
    if !workspace.is_absolute() || !resolved.starts_with(workspace) {
        return Err("Graph文件必须位于当前连接工作区内；选择文件不会扩大音频访问范围。".into());
    }
    Ok(resolved)
}

pub(crate) fn load_graph_file(workspace: &Path, path: &str) -> Result<GraphDocument, String> {
    let path = json_path(workspace, path, true)?;
    let file = File::open(&path).map_err(|e| format!("读取Graph失败：{e}"))?;
    let mut bytes = Vec::new();
    file.take((MAX_REQUEST + 1) as u64).read_to_end(&mut bytes).map_err(|e| format!("读取Graph失败：{e}"))?;
    let graph = strict_json(&bytes)?;
    if graph.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("Graph schema_version必须采用整数1，不能使用1.0或1e0".into());
    }
    Ok(GraphDocument { path: path.to_string_lossy().into_owned(), graph })
}

pub(crate) fn save_graph_file(workspace: &Path, path: &str, graph: &Value) -> Result<SavedGraph, String> {
    let bytes = serde_json::to_vec_pretty(graph).map_err(|e| e.to_string())?;
    strict_json(&bytes)?;
    if graph.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("Graph schema_version必须采用整数1".into());
    }
    let path = json_path(workspace, path, false)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path)
        .map_err(|e| format!("无法创建新Graph文件（不覆盖已有文件，请另选名称）：{e}"))?;
    file.write_all(&bytes).and_then(|_| file.flush())
        .map_err(|e| format!("写入Graph失败，可能留下部分新文件：{e}"))?;
    Ok(SavedGraph { path: path.to_string_lossy().into_owned() })
}
