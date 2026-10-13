//! 写前敏感扫描：密码/身份证/手机号/银行卡/token/私钥，默认拒绝入库。
//! 命中即拒（不是脱敏改写）——提示用户改存位置指针（如「密码在 ~/.env」）。
//! 想强制入库暂不提供口子：这是嵌入式明文库，宁严勿松。

/// 扫描文本，返回命中种类（空表 = 干净）
pub fn scan(text: &str) -> Vec<&'static str> {
    let mut hits = Vec::new();
    let t = text;

    // PEM 私钥块
    if t.contains("-----BEGIN") && (t.contains("PRIVATE KEY") || t.contains("RSA PRIVATE")) {
        hits.push("PEM私钥");
    }
    // JWT（eyJ 开头的三段 base64url）
    if let Some(jwt) = find_jwt(t) {
        let _ = jwt;
        hits.push("JWT");
    }
    // 常见 token 前缀
    for (pat, name) in [
        ("sk-", "API密钥(sk-)"),
        ("ghp_", "GitHub token"),
        ("gho_", "GitHub token"),
        ("github_pat_", "GitHub PAT"),
        ("AKIA", "AWS密钥"),
        ("xox", "Slack token"),
    ] {
        if t.contains(pat) {
            // sk- 后须跟足够长的字母数字，防误伤普通词
            if pat == "sk-" {
                if let Some(p) = t.find("sk-") {
                    let tail = &t[p + 3..];
                    let run: String = tail.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                    if run.len() >= 16 {
                        hits.push(name);
                    }
                }
            } else {
                hits.push(name);
            }
        }
    }
    // key=value / password: xxx 赋值式口令（明文口令）
    if has_plaintext_password(t) {
        hits.push("明文口令");
    }
    // 身份证 18 位（末位可 X）
    if find_cn_id(t).is_some() {
        hits.push("身份证号");
    }
    // 银行卡 16-19 位连续数字（Luhn 校验，防误伤长订单号）
    if find_bank_card(t).is_some() {
        hits.push("银行卡号");
    }
    // CN 手机号 1[3-9]xxxxxxxxx（11 位，带边界）
    if find_cn_phone(t).is_some() {
        hits.push("手机号");
    }
    hits.dedup();
    hits
}

fn find_jwt(t: &str) -> Option<()> {
    // eyJ 开头，后接 ≥10 个 base64url 字符，再 '.' 再 eyJ 或非空段
    let bytes = t.as_bytes();
    let mut i = 0;
    while i + 10 < bytes.len() {
        if bytes[i..].starts_with(b"eyJ") {
            let rest = &t[i + 3..];
            let seg1: usize = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').count();
            if seg1 >= 10 {
                let after = &rest[seg1..];
                if after.starts_with('.') {
                    let seg2: usize = after[1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').count();
                    if seg2 >= 5 {
                        return Some(());
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn has_plaintext_password(t: &str) -> bool {
    // 形如 password=xxx / passwd: xxx / 密码：xxx / 密码是xxx（赋值右侧非空、非纯占位符）
    let lower = t.to_ascii_lowercase();
    for key in [
        "password=", "password：", "password:", "password is ",
        "passwd=", "passwd:", "pwd=", "pwd:",
        "密码=", "密码：", "密码:", "密码是", "密碼是",
        "口令=", "口令：", "口令:", "口令是",
    ] {
        let pat = if key.chars().next().is_some_and(|c| c.is_ascii()) && key.ends_with('=') || key.ends_with(':') || key.ends_with('：') {
            key
        } else {
            key
        };
        let hay = if pat.chars().next().is_some_and(|c| c.is_ascii()) { &lower } else { t };
        if let Some(p) = hay.find(pat) {
            let val = hay[p + pat.len()..].trim_start();
            let val = val.trim_start_matches(['"', '\'']);
            let word: String = val.chars().take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'' && *c != ')' && *c != ';').collect();
            // 排除占位符、疑问词与显然非密码的
            let placeholder = ["none", "null", "true", "false", "<password>", "${password}", "***", "xxx"];
            let interrogative = ["什么", "啥", "多少", "几位", "忘了", "不告诉你", "不知道", "没设", "没有"];
            if word.len() >= 4
                && !placeholder.contains(&word.as_str())
                && !interrogative.iter().any(|w| word.starts_with(w))
            {
                return true;
            }
        }
    }
    false
}

/// 简化 Luhn
fn luhn_ok(num: &str) -> bool {
    let digits: Vec<u32> = num.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() < 16 {
        return false;
    }
    let mut sum = 0;
    for (i, d) in digits.iter().rev().enumerate() {
        let mut d = *d;
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum % 10 == 0
}

fn find_bank_card(t: &str) -> Option<String> {
    // 抽连续数字段（≥16），Luhn 通过才认
    for seg in digit_runs(t, 16, 19) {
        if luhn_ok(&seg) {
            return Some(seg);
        }
    }
    None
}

/// 18 位身份证：17 数字 + 末位数字或 X；前 6 位 19/20 开头的年份段合理性弱校验
fn find_cn_id(t: &str) -> Option<String> {
    for seg in digit_runs(t, 17, 18) {
        let bytes = seg.as_bytes();
        if bytes.len() == 17 {
            // 还要跟一个 X/x 才完整
            continue;
        }
        if bytes.len() == 18 {
            let year: u32 = seg[6..10].parse().unwrap_or(0);
            if (1900..=2100).contains(&year) {
                return Some(seg);
            }
        }
    }
    // 17 位数字紧跟 X
    let b = t.as_bytes();
    for i in 0..b.len() {
        if b[i].is_ascii_digit() {
            let run: usize = (i..b.len()).take_while(|&j| b[j].is_ascii_digit()).count();
            if run == 17 && i + 17 < b.len() && (b[i + 17] == b'X' || b[i + 17] == b'x') {
                let seg = &t[i..i + 17];
                let year: u32 = seg[6..10].parse().unwrap_or(0);
                if (1900..=2100).contains(&year) {
                    return Some(format!("{seg}X"));
                }
            }
        }
    }
    None
}

/// 11 位手机号：1[3-9]xxxxxxxxx，前后无其他数字（全程 bytes，避免多字节字符边界 panic）
fn find_cn_phone(t: &str) -> Option<String> {
    let b = t.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'1' && i + 11 <= b.len() {
            let seg = &b[i..i + 11];
            if seg.iter().all(|c| c.is_ascii_digit()) {
                let second = seg[1];
                if (b'3'..=b'9').contains(&second) {
                    // 前后不接数字（防 12 位长数字中段误中）
                    let before_ok = i == 0 || !b[i - 1].is_ascii_digit();
                    let after_ok = i + 11 == b.len() || !b[i + 11].is_ascii_digit();
                    if before_ok && after_ok {
                        return Some(String::from_utf8_lossy(seg).into_owned());
                    }
                }
            }
        }
    }
    None
}

/// 抽 [min,max] 位连续纯数字段
fn digit_runs(t: &str, min: usize, max: usize) -> Vec<String> {
    let b = t.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let run: usize = (i..b.len()).take_while(|&j| b[j].is_ascii_digit()).count();
            if run >= min {
                // 整段超长时按窗滑取（银行卡可嵌在更长数字串里）
                if run > max {
                    for s in i..=i + run - max {
                        out.push(t[s..s + max].to_string());
                    }
                } else {
                    out.push(t[i..i + run].to_string());
                }
            }
            i += run;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_passes() {
        assert!(scan("金秤回测：趋势回踩 H1 做多，止损 4155，单笔风险 1%").is_empty());
        assert!(scan("2026-10-06 研判结论：均值回归做多，点位 4183.50").is_empty());
    }

    #[test]
    fn password_hits() {
        assert!(scan("db_password=secret123").contains(&"明文口令"));
        assert!(scan("密码：hunter2ok").contains(&"明文口令"));
        assert!(!scan("password=none").contains(&"明文口令"));
    }

    #[test]
    fn id_card_hits() {
        assert!(scan("身份证号 11010519491231002X").contains(&"身份证号"));
        assert!(scan("11010519491231002X").contains(&"身份证号"));
        // 19 位纯数字不误判身份证
        assert!(!scan("20261006123456789").contains(&"身份证号"));
    }

    #[test]
    fn colloquial_password_hits() {
        assert!(scan("我的密码是hunter2secret").contains(&"明文口令"));
        assert!(scan("admin 的口令是 x9y8z7w6").contains(&"明文口令"));
        assert!(!scan("他问我密码是什么，我没说").contains(&"明文口令"));
    }

    #[test]
    fn phone_hits_with_boundaries() {
        assert!(scan("联系我 13812345678 谢谢").contains(&"手机号"));
        assert!(!scan("订单 20261006138123456789012 长数字").contains(&"手机号"));
    }

    #[test]
    fn bank_card_luhn() {
        // 6222021234564321789 不一定过 Luhn——用已知过校验的：6222021234564321 671? 换官方测试号
        assert!(scan("卡号 4111111111111111").contains(&"银行卡号")); // Visa 测试号
        assert!(!scan("编号 411111111111111").contains(&"银行卡号")); // 15 位不够
    }

    #[test]
    fn token_prefixes() {
        assert!(scan("api key: sk-abcdefghijklmnop123456").contains(&"API密钥(sk-)"));
        // sk- 后缀太短不误伤（ask-xxx 这类普通词）
        assert!(!scan("ask-你好世界").contains(&"API密钥(sk-)"));
        assert!(scan("ghp_1234567890abcdef1234567890abcdef12").contains(&"GitHub token"));
        assert!(scan("-----BEGIN RSA PRIVATE KEY-----\nMIIEpA").contains(&"PEM私钥"));
    }

    #[test]
    fn jwt_hits() {
        assert!(scan("token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.abc").contains(&"JWT"));
        assert!(!scan("eyJ是一个普通词").contains(&"JWT"));
    }
}