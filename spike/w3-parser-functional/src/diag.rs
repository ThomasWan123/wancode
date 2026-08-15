//! 诊断：PDF 是否**本身没有文字层**（扫描件），还是抽取器读不出。
//!
//! 注意资源**可继承**：页字典没有 /Resources 时要沿 Pages 树上取。
//! lopdf 的 get_page_resources 返回 (本页资源, 继承资源 id 列表)，两者都要看，
//! 否则会得出「0 字体 0 图像」这种自相矛盾的结论（我第一版就是这样）。
fn dict_has<'a>(doc: &'a lopdf::Document, d: &'a lopdf::Dictionary, key: &[u8]) -> bool {
    d.get(key).is_ok() || {
        // 有些实现把资源再套一层引用
        d.get(key).ok().and_then(|v| doc.dereference(v).ok()).is_some()
    }
}

fn main() {
    let p = std::env::args().nth(1).expect("需要 pdf 路径");
    let doc = lopdf::Document::load(&p).expect("加载失败");
    let (mut with_font, mut with_text_op, mut with_image, mut total) = (0, 0, 0, 0);
    let mut text_op_count = 0usize;
    for (_, &pid) in doc.get_pages().iter() {
        total += 1;
        // —— 资源：本页 + 继承 ——
        let (own, inherited_ids) = doc.get_page_resources(pid).unwrap_or((None, Vec::new()));
        let mut dicts: Vec<lopdf::Dictionary> = Vec::new();
        if let Some(d) = own { dicts.push(d.clone()); }
        for id in inherited_ids {
            if let Ok(lopdf::Object::Dictionary(d)) = doc.get_object(id) { dicts.push(d.clone()); }
        }
        let mut font = false; let mut image = false;
        for d in &dicts {
            if dict_has(&doc, d, b"Font") { font = true; }
            if let Ok(xo) = d.get(b"XObject").and_then(|v| doc.dereference(v).map(|(_, o)| o)) {
                if let Ok(xd) = xo.as_dict() {
                    for (_, v) in xd.iter() {
                        if let Ok((_, o)) = doc.dereference(v) {
                            if let Ok(s) = o.as_stream() {
                                if s.dict.get(b"Subtype").ok().and_then(|st| st.as_name().ok())
                                    == Some(b"Image".as_ref()) { image = true; }
                            }
                        }
                    }
                }
            }
        }
        if font { with_font += 1; }
        if image { with_image += 1; }
        if let Ok(content) = doc.get_and_decode_page_content(pid) {
            let n = content.operations.iter()
                .filter(|op| matches!(op.operator.as_str(), "Tj" | "TJ" | "'" | "\""))
                .count();
            if n > 0 { with_text_op += 1; text_op_count += n; }
        }
    }
    let verdict = if with_text_op == 0 {
        "无文本算子 —— 无文字层（扫描件/纯图像），任何抽取器都取不到，需 OCR"
    } else if with_font == 0 {
        "有文本算子但无字体资源 —— 文本无法映射为字符，抽取器取不出属预期"
    } else {
        "有文字层且有字体 —— 抽取器取不出即为选型问题"
    };
    println!("{}", serde_json::json!({
        "pages": total,
        "pages_with_font_resource": with_font,
        "pages_with_text_operators": with_text_op,
        "text_operator_count": text_op_count,
        "pages_with_image_xobject": with_image,
        "verdict": verdict,
    }));
}
