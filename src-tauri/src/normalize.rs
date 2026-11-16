//! Chuẩn hoá văn bản cho tìm kiếm: bỏ dấu tiếng Việt, `đ` → `d`, viết thường.
//!
//! "Kết quả" và "ket qua" có cùng dạng gấp (fold) → người dùng gõ không dấu vẫn tìm được.
//! `fold` trả kèm bảng ánh xạ vị trí để tô sáng đoạn trích trên văn bản GỐC.

use unicode_normalization::char::{decompose_canonical, is_combining_mark};

/// Gấp 1 ký tự (đã NFC) → 0..n ký tự, gọi `push` cho từng ký tự kết quả.
fn fold_char(c: char, mut push: impl FnMut(char)) {
    decompose_canonical(c, |d| {
        // Bỏ dấu kết hợp. Lưu ý: crate chỉ có `General_Category=Mark` (Mn + Mc + Me);
        // với tiếng Việt / Latin thì chỉ có Mn nên kết quả như nhau.
        if is_combining_mark(d) {
            return;
        }
        let d = match d {
            'đ' => 'd',
            'Đ' => 'D',
            other => other,
        };
        // `to_lowercase` có thể ra nhiều ký tự → không bao giờ giả định 1:1.
        for l in d.to_lowercase() {
            if !is_combining_mark(l) {
                push(l);
            }
        }
    });
}

/// Gấp `s` để so khớp. Trả `(folded, map)` với `map[i]` = chỉ số ký tự (char, không phải byte)
/// trong `s` gốc sinh ra ký tự thứ `i` của `folded`. `map.len() == folded.chars().count()`.
///
/// Quy trình tương đương "NFC rồi NFD từng ký tự": vì NFD(NFC(x)) ≡ NFD(x) và mọi dấu kết hợp
/// đều bị bỏ, ta xử lý trực tiếp từng ký tự gốc — nhờ vậy `map` trỏ đúng vào chuỗi người dùng
/// đưa vào kể cả khi chuỗi đó đang ở dạng NFD (dấu tách rời).
pub fn fold(s: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(s.len());
    let mut map = Vec::with_capacity(s.len());
    for (idx, c) in s.chars().enumerate() {
        fold_char(c, |f| {
            out.push(f);
            map.push(idx);
        });
    }
    (out, map)
}

/// Tách văn bản đã gấp thành các từ (chuỗi ký tự chữ/số liên tiếp), kèm vị trí char `[start, end)`.
/// Gần với tokenizer `unicode61` của FTS5 (ký tự chữ/số = token, còn lại = phân cách).
pub fn word_spans(folded: &[char]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in folded.iter().enumerate() {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                spans.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        spans.push((s, folded.len()));
    }
    spans
}

/// Biến câu tìm kiếm bất kỳ thành biểu thức FTS5 an toàn: mỗi token (tách theo khoảng trắng)
/// thành 1 phrase trong ngoặc kép, dấu `"` bên trong được nhân đôi → `OR`, `*`, `-`, `:`,
/// `NEAR(...)` của người dùng chỉ là chữ thường, không bao giờ gây lỗi cú pháp.
/// Token không có ký tự chữ/số nào (vd `*`, `-`) bị bỏ vì FTS5 không tạo được term từ nó.
/// Các phrase nối bằng khoảng trắng = AND ngầm định. Input rỗng → chuỗi rỗng.
pub fn fold_query(q: &str) -> String {
    let (folded, _) = fold(q);
    folded
        .split_whitespace()
        .filter(|tok| tok.chars().any(char::is_alphanumeric))
        .map(|tok| format!("\"{}\"", tok.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_vietnamese() {
        assert_eq!(fold("Kết quả").0, "ket qua");
        assert_eq!(fold("đang").0, "dang");
        assert_eq!(fold("ĐANG").0, "dang");
        assert_eq!(fold("Đường Đi Ở Đâu").0, "duong di o dau");
        // Chuỗi đã ASCII thường thì giữ nguyên, map là đồng nhất.
        let (f, m) = fold("abc 1");
        assert_eq!(f, "abc 1");
        assert_eq!(m, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn nfd_input_maps_to_original_chars() {
        // "Kết quả" dạng NFD: e + U+0302 (mũ) + U+0301 (sắc), a + U+0309 (hỏi).
        let nfd = "Ke\u{302}\u{301}t qua\u{309}";
        let (f, m) = fold(nfd);
        assert_eq!(f, "ket qua");
        assert_eq!(f, fold("Kết quả").0, "NFC và NFD gấp ra cùng kết quả");
        // 't' là ký tự gốc thứ 4 (sau K, e, 2 dấu)
        assert_eq!(m, vec![0, 1, 4, 5, 6, 7, 8]);
        // Trộn NFC + NFD trong cùng chuỗi
        assert_eq!(fold("Kết qua\u{309} đa\u{300}ng").0, "ket qua dang");
    }

    #[test]
    fn dotted_capital_i_and_expansion() {
        // İ (U+0130) → NFD: I + U+0307 → bỏ dấu → "i"
        let (f, m) = fold("İstanbul");
        assert_eq!(f, "istanbul");
        assert_eq!(m.len(), f.chars().count());
        assert_eq!(m[0], 0);
        assert_eq!(m[1], 1);
        // Ký tự gấp có thể ít/nhiều hơn gốc: map luôn dài bằng chuỗi gấp và không giảm.
        for s in ["ǅ ß Ω Å ﬀ", "e\u{301}\u{301}\u{301}x", "😀 Ấn Độ"] {
            let (f, m) = fold(s);
            assert_eq!(m.len(), f.chars().count(), "{s}");
            assert!(m.windows(2).all(|w| w[0] <= w[1]), "{s}");
            assert!(m.iter().all(|&i| i < s.chars().count()), "{s}");
        }
    }

    #[test]
    fn query_is_always_quoted() {
        assert_eq!(fold_query("Kết quả"), "\"ket\" \"qua\"");
        assert_eq!(fold_query("a OR b"), "\"a\" \"or\" \"b\"");
        assert_eq!(fold_query("say \"hi\""), "\"say\" \"\"\"hi\"\"\"");
        assert_eq!(fold_query("col:value"), "\"col:value\"");
        assert_eq!(fold_query("pre* -x"), "\"pre*\" \"-x\"");
        assert_eq!(fold_query("\""), "");
        assert_eq!(fold_query("*"), "");
        assert_eq!(fold_query(" - : "), "");
        assert_eq!(fold_query(""), "");
        assert_eq!(fold_query("   \t\n"), "");
    }

    #[test]
    fn word_spans_split_on_non_alnum() {
        let chars: Vec<char> = "ket-qua  v2!".chars().collect();
        assert_eq!(word_spans(&chars), vec![(0, 3), (4, 7), (9, 11)]);
    }
}
