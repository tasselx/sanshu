#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WechatReply {
    pub continue_requested: bool,
    pub selected_options: Vec<String>,
    pub user_input: Option<String>,
}

pub fn request_short_code(request_id: &str) -> String {
    let code: String = request_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(6)
        .collect();
    if code.is_empty() {
        "ZHI".to_string()
    } else {
        code.to_ascii_uppercase()
    }
}

pub fn parse_wechat_reply(
    input: &str,
    expected_code: &str,
    options: &[String],
) -> Option<WechatReply> {
    let mut lines: Vec<&str> = input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }

    if let Some(code) = lines[0].strip_prefix('#') {
        if !code.trim().eq_ignore_ascii_case(expected_code) {
            return None;
        }
        lines.remove(0);
    }
    if lines.is_empty() {
        return None;
    }

    if lines.len() == 1 && lines[0] == "继续" {
        return Some(WechatReply {
            continue_requested: true,
            selected_options: Vec::new(),
            user_input: None,
        });
    }

    if options.is_empty() {
        let joined = lines.join("\n");
        let text = joined
            .strip_prefix("回复：")
            .or_else(|| joined.strip_prefix("回复:"))
            .unwrap_or(&joined)
            .trim()
            .to_string();
        return (!text.is_empty()).then_some(WechatReply {
            continue_requested: false,
            selected_options: Vec::new(),
            user_input: Some(text),
        });
    }

    let joined = lines.join("\n");
    let mut selection = None;
    let mut supplement = None;
    for (index, line) in lines.iter().enumerate() {
        if let Some(value) = strip_field(line, &["选择：", "选择:"]) {
            selection = Some(value.to_string());
        } else if let Some(value) = strip_field(line, &["补充：", "补充:"]) {
            let mut values = vec![value];
            values.extend(lines[index + 1..].iter().copied());
            supplement = Some(values.join("\n").trim().to_string());
            break;
        }
    }

    if selection.is_none() && lines.len() == 1 {
        let (left, right) = joined.split_once('+').unwrap_or((&joined, ""));
        selection = Some(left.trim().to_string());
        if !right.trim().is_empty() {
            supplement = Some(right.trim().to_string());
        }
    }

    let selected_options = parse_selection(selection.as_deref().unwrap_or_default(), options)?;
    let user_input = supplement.filter(|value| !value.trim().is_empty());
    if selected_options.is_empty() && user_input.is_none() {
        return None;
    }

    Some(WechatReply {
        continue_requested: false,
        selected_options,
        user_input,
    })
}

fn strip_field<'a>(line: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix).map(str::trim))
}

fn parse_selection(value: &str, options: &[String]) -> Option<Vec<String>> {
    if value.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut selected = Vec::new();
    for token in value
        .split(|ch| ch == ',' || ch == '，')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let option = if token.len() == 1 {
            let letter = token.chars().next()?.to_ascii_uppercase();
            if !letter.is_ascii_uppercase() {
                return None;
            }
            let index = letter as usize - 'A' as usize;
            options.get(index)?
        } else {
            options.iter().find(|option| option.trim() == token)?
        };
        if !selected.contains(option) {
            selected.push(option.clone());
        }
    }
    Some(selected)
}
