//! 根因诊断：文本算子取不出字符，是因为字体缺 ToUnicode 映射，还是别的。
//! 无 ToUnicode 的子集嵌入字体，glyph id 无法反查字符——这是"文本抽取弱"
//! 的经典成因，且**换任何抽取器都一样**（除非做 OCR 或字形反查）。
fn main() {
    let p = std::env::args().nth(1).expect("需要 pdf 路径");
    let doc = lopdf::Document::load(&p).expect("加载失败");
    let (mut fonts, mut with_tounicode, mut typ3, mut simple, mut type0) = (0, 0, 0, 0, 0);
    for (_, &pid) in doc.get_pages().iter() {
        let (own, inh) = doc.get_page_resources(pid).unwrap_or((None, Vec::new()));
        let mut dicts: Vec<lopdf::Dictionary> = Vec::new();
        if let Some(d) = own { dicts.push(d.clone()); }
        for id in inh { if let Ok(lopdf::Object::Dictionary(d)) = doc.get_object(id) { dicts.push(d.clone()); } }
        for d in &dicts {
            if let Ok(fd) = d.get(b"Font").and_then(|v| doc.dereference(v).map(|(_, o)| o)) {
                if let Ok(fdict) = fd.as_dict() {
                    for (_, v) in fdict.iter() {
                        if let Ok((_, o)) = doc.dereference(v) {
                            if let Ok(f) = o.as_dict() {
                                fonts += 1;
                                if f.get(b"ToUnicode").is_ok() { with_tounicode += 1; }
                                match f.get(b"Subtype").ok().and_then(|s| s.as_name().ok()) {
                                    Some(b"Type0") => type0 += 1,
                                    Some(b"Type3") => typ3 += 1,
                                    _ => simple += 1,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!("{}", serde_json::json!({
        "font_refs_seen": fonts,
        "with_tounicode": with_tounicode,
        "type0_cid": type0, "type3": typ3, "simple": simple,
        "root_cause": if fonts > 0 && with_tounicode == 0 {
            "字体全部缺 ToUnicode —— glyph 无法反查字符，属 PDF 本身不可抽取，换抽取器也救不了（需 OCR）"
        } else if with_tounicode > 0 {
            "存在 ToUnicode —— 字符本可反查，lopdf 未能利用即为选型问题"
        } else { "未见字体引用" },
    }));
}
