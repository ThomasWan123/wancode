//! 引擎兼容契约的直接证伪测试（#126 B4 复核 P1）。
//!
//! 4a/4b 合规 mock 都显式带 `"object"` 字段，覆盖不到 sampling-types 里
//! `#[serde(default)]` 的缺失容忍——删掉该属性现有套件依然全绿。这里用
//! "缺 object 的最小响应体必须可解析"直接钉住契约：智谱 GLM-4V 系列
//! （含流式 chunk）不带 object，严格反序列化会把合法响应判为解析失败。
//! 两条分开：Response 与 Chunk 各自的属性可独立回归。

use xai_grok_sampling_types::{ChatCompletionChunk, ChatCompletionResponse};

#[test]
fn chat_completion_response_parses_without_object_field() {
    let body = r#"{
        "id": "resp-1",
        "created": 1700000000,
        "model": "glm-4v",
        "choices": []
    }"#;
    let parsed: ChatCompletionResponse =
        serde_json::from_str(body).expect("缺 object 的响应体必须可解析（GLM-4V 兼容契约）");
    assert_eq!(parsed.object, "", "缺失时应落到 String::default()");
}

#[test]
fn chat_completion_chunk_parses_without_object_field() {
    let body = r#"{
        "id": "chunk-1",
        "created": 1700000000,
        "model": "glm-4v",
        "choices": []
    }"#;
    let parsed: ChatCompletionChunk =
        serde_json::from_str(body).expect("缺 object 的流式 chunk 必须可解析（GLM-4V 兼容契约）");
    assert_eq!(parsed.object, "", "缺失时应落到 String::default()");
}
