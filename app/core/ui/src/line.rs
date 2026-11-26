//! A bar's Yos at the small size, when they do not fit in one line at the bar's size: a group's senders, newest
//! first, flowing over two lines ("2× Yo from TOM, 3× ANNA," / "4× PETER, LUKAS +2"), or one sender whose line is
//! too long ("3× Yo from" / "MAXIMILIAN_MUELL"). The platform measures the pieces at the small size and renders the
//! two lines; the core decides what goes where. Names are never cut; whatever does not fit ends the second line as
//! " +N".
use crate::{arrival_line, counted, Language};

/// The pieces a platform measures, in the bar's small font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flow {
    /// The newest sender's Yos, with its space: "2× Yo from ", "Yo von ".
    pub prefix: String,
    /// One per sender, newest first: the newest's name ("TOM"), then each with its count ("3× ANNA", "LUKAS").
    pub tokens: Vec<String>,
    /// Between two names: ", ".
    pub separator: String,
    /// The end of a first line the second line continues: ",".
    pub comma: String,
    /// `more[k]` is " +{k+1}": how many names did not fit.
    pub more: Vec<String>,
}

/// The widths the platform measured for a `Flow`'s pieces, in one unit (points, pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct FlowWidths {
    pub prefix: f32,
    pub tokens: Vec<f32>,
    pub separator: f32,
    pub comma: f32,
    pub more: Vec<f32>,
}

/// The senders' Yos on the bar, newest first: name and count. One sender is the bar's own arrival line at the
/// small size ("3× Yo from MAXIMILIAN_MUELL").
pub fn sender_flow(senders: &[(String, u32)], lang: Language) -> Flow {
    let first = senders.first().map_or(1, |(_, count)| *count);
    Flow {
        prefix: arrival_line(&counted(i64::from(first)), "", lang),
        tokens: senders
            .iter()
            .enumerate()
            .map(|(i, (name, count))| if i > 0 && *count > 1 { format!("{count}× {name}") } else { name.clone() })
            .collect(),
        separator: ", ".into(),
        comma: ",".into(),
        more: (1..senders.len()).map(|n| format!(" +{n}")).collect(),
    }
}

/// The two lines (the second may be empty) for the room the bar has. Widths add up (kerning between pieces is
/// within the room's margin). Names are whole: when the first does not fit beside the prefix, the prefix stands
/// alone on the first line and the names start on the second. A name wider than the room even alone is still
/// placed whole (the platform shrinks that line to fit); whatever else does not fit ends the second line as " +N".
pub fn flow_two_lines(flow: &Flow, widths: &FlowWidths, room: f32) -> [String; 2] {
    let n = flow.tokens.len();
    let width = |i: usize| widths.tokens.get(i).copied().unwrap_or(0.0);
    let more = |left: usize| if left == 0 { 0.0 } else { widths.more.get(left - 1).copied().unwrap_or(0.0) };
    let mut first = flow.prefix.clone();
    let mut used = widths.prefix;
    let mut placed = 0;
    while placed < n {
        let add = if placed == 0 { 0.0 } else { widths.separator } + width(placed);
        let comma = if placed + 1 < n { widths.comma } else { 0.0 };
        if used + add + comma > room {
            break;
        }
        if placed > 0 {
            first.push_str(&flow.separator);
        }
        first.push_str(&flow.tokens[placed]);
        used += add;
        placed += 1;
    }
    let mut second = String::new();
    if placed == 0 {
        first.truncate(first.trim_end().len());
    } else if placed < n {
        first.push_str(&flow.comma);
    }
    let start = placed;
    let mut used = 0.0;
    while placed < n {
        let add = if placed == start { 0.0 } else { widths.separator } + width(placed);
        // The first line holds no name: the second always holds one.
        let forced = start == 0 && placed == 0;
        if !forced && used + add + more(n - placed - 1) > room {
            break;
        }
        if placed > start {
            second.push_str(&flow.separator);
        }
        second.push_str(&flow.tokens[placed]);
        used += add;
        placed += 1;
    }
    if placed < n {
        let left = n - placed;
        let tail = flow.more.get(left - 1).cloned().unwrap_or_else(|| format!(" +{left}"));
        second.push_str(if second.is_empty() { tail.trim_start() } else { &tail });
    }
    [first, second]
}

/// A bar's name as shown: a group's with one "#" ("#WG"), whether the name brings its own or not.
pub fn bar_title(name: &str, group: bool) -> String {
    if group {
        format!("#{}", name.strip_prefix('#').unwrap_or(name))
    } else {
        name.to_string()
    }
}

/// A bar's title in words, for two small lines when it does not fit in one ("Yo Request from" /
/// "MAXIMILIAN_MUELL"). Every bar title: the bar's size if it fits, else the small size, else two small lines.
pub fn title_words(text: &str) -> Vec<String> {
    text.split(' ').filter(|word| !word.is_empty()).map(str::to_string).collect()
}

/// The title's words over two lines: as many whole words on the first as fit (at least one), the rest on the
/// second, never cut (a line wider than the room is shrunk by the platform).
pub fn wrap_two_lines(words: &[String], widths: &[f32], space: f32, room: f32) -> [String; 2] {
    let width = |i: usize| widths.get(i).copied().unwrap_or(0.0);
    let mut used = 0.0;
    let mut split = 0;
    while split < words.len() {
        let add = if split == 0 { 0.0 } else { space } + width(split);
        if split > 0 && used + add > room {
            break;
        }
        used += add;
        split += 1;
    }
    [words[..split].join(" "), words[split..].join(" ")]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn senders(list: &[(&str, u32)]) -> Vec<(String, u32)> {
        list.iter().map(|(name, count)| ((*name).to_string(), *count)).collect()
    }
    /// Every character one unit wide: the measured widths are the strings' lengths.
    fn widths(flow: &Flow) -> FlowWidths {
        let w = |s: &String| s.chars().count() as f32;
        FlowWidths {
            prefix: w(&flow.prefix),
            tokens: flow.tokens.iter().map(w).collect(),
            separator: w(&flow.separator),
            comma: w(&flow.comma),
            more: flow.more.iter().map(w).collect(),
        }
    }
    fn flow(list: &[(&str, u32)], room: f32) -> [String; 2] {
        let flow = sender_flow(&senders(list), Language::English);
        flow_two_lines(&flow, &widths(&flow), room)
    }
    /// As `flow`, and no line wider than the room.
    fn lines(list: &[(&str, u32)], room: f32) -> [String; 2] {
        let out = flow(list, room);
        for line in &out {
            assert!(line.chars().count() as f32 <= room, "{line:?} over {room}");
        }
        out
    }

    #[test]
    fn the_count_goes_before_the_name_and_speaks_german() {
        let flow = sender_flow(&senders(&[("TOM", 2), ("ANNA", 3), ("LUKAS", 1)]), Language::English);
        assert_eq!(flow.prefix, "2× Yo from ");
        assert_eq!(flow.tokens, ["TOM", "3× ANNA", "LUKAS"]);
        assert_eq!(flow.more, [" +1", " +2"]);
        assert_eq!(sender_flow(&senders(&[("MIA", 1)]), Language::German).prefix, "Yo von ");
        assert_eq!(sender_flow(&senders(&[("MIA", 4)]), Language::German).prefix, "4× Yo von ");
    }

    /// Whole lines for a list and a room: the owner's example, the last name instead of "+1", a long first name
    /// leaving the prefix alone on the first line (it ran over both edges once), one sender at the small size.
    #[test]
    fn senders_flow_over_two_lines() {
        type Case<'a> = (&'a [(&'a str, u32)], f32, [&'a str; 2]);
        let cases: [Case; 7] = [
            (&[("TOM", 1), ("IDA", 2)], 40.0, ["Yo from TOM, 2× IDA", ""]),
            (
                &[("TOM", 2), ("ANNA", 3), ("PETER", 4), ("LUKAS", 1), ("SAM", 1), ("KAI", 1)],
                24.0,
                ["2× Yo from TOM, 3× ANNA,", "4× PETER, LUKAS, SAM +1"],
            ),
            (
                &[("TOM", 1), ("IDA", 1), ("BEN", 1), ("SAM", 1), ("KAI", 1)],
                22.0,
                ["Yo from TOM, IDA, BEN,", "SAM, KAI"],
            ),
            (&[("ALEXANDRAWINTERS", 2), ("IDA", 1)], 27.0, ["2× Yo from", "ALEXANDRAWINTERS, IDA"]),
            (
                &[("ALEXANDRAWINTERS", 2), ("KONSTANTIN_BERG_", 2), ("IDA", 1)],
                22.0,
                ["2× Yo from", "ALEXANDRAWINTERS +2"],
            ),
            (&[("MAXIMILIAN_MUELL", 3)], 30.0, ["3× Yo from MAXIMILIAN_MUELL", ""]),
            (&[("MAXIMILIAN_MUELL", 3)], 20.0, ["3× Yo from", "MAXIMILIAN_MUELL"]),
        ];
        for (list, room, want) in cases {
            assert_eq!(lines(list, room), want, "{list:?} in {room}");
        }
    }

    #[test]
    fn a_second_line_without_room_for_a_name_is_only_more() {
        let flow = sender_flow(&senders(&[("A", 1), ("LONGNAMEXX", 1), ("C", 1)]), Language::English);
        let mut widths = widths(&flow);
        widths.tokens[1] = 100.0;
        assert_eq!(flow_two_lines(&flow, &widths, 12.0), ["Yo from A,", "+2"]);
    }

    #[test]
    fn sixteen_character_names_with_counts_never_overflow() {
        let long = ["MAXIMILIAN_MUELL", "ALEXANDRAWINTERS", "KONSTANTIN_BERG_", "SEBASTIAN_KRAUSE", "CHRISTOPHERJONES"];
        // From 19 on: one name and " +4"; narrower, the platform shrinks the line.
        for room in 19..60 {
            for n in 1..=long.len() {
                for count in [1, 12] {
                    let list: Vec<_> = long[..n].iter().map(|name| (*name, count)).collect();
                    let out = lines(&list, room as f32);
                    assert!(out.concat().contains("MAXIMILIAN_MUELL"), "the newest name, whole: {out:?}");
                    assert!(!out[0].is_empty());
                }
            }
        }
    }

    #[test]
    fn a_name_too_wide_alone_is_placed_whole_never_cut() {
        assert_eq!(flow(&[("MAXIMILIAN_MUELL", 1), ("IDA", 1)], 10.0), ["Yo from", "MAXIMILIAN_MUELL +1"]);
        assert_eq!(flow(&[("MAXIMILIAN_MUELL", 3)], 10.0), ["3× Yo from", "MAXIMILIAN_MUELL"]);
    }

    #[test]
    fn a_group_title_has_one_hash() {
        assert_eq!(bar_title("WG", true), "#WG");
        assert_eq!(bar_title("#BOTS", true), "#BOTS");
        assert_eq!(bar_title("MIA", false), "MIA");
    }

    #[test]
    fn titles_wrap_whole_words_over_two_lines() {
        let wrap = |text: &str, room: f32| {
            let words = title_words(text);
            let widths: Vec<f32> = words.iter().map(|w| w.chars().count() as f32).collect();
            wrap_two_lines(&words, &widths, 1.0, room)
        };
        assert_eq!(wrap("Yo Request from MAXIMILIAN_MUELL", 20.0), ["Yo Request from", "MAXIMILIAN_MUELL"]);
        assert_eq!(wrap("Yo Request from CHRISTOPHERJONES", 32.0), ["Yo Request from CHRISTOPHERJONES", ""]);
        assert_eq!(wrap("Yo Request: #LANGENAMENGRUPPE", 16.0), ["Yo Request:", "#LANGENAMENGRUPPE"]);
        // A 16-character name alone is one word: never cut, at worst shrunk by the platform.
        assert_eq!(wrap("ALEXANDRAWINTERS", 10.0), ["ALEXANDRAWINTERS", ""]);
        assert_eq!(title_words("  Sent  3× Yo! "), ["Sent", "3×", "Yo!"]);
    }

    #[test]
    fn sixteen_long_names_at_a_real_room() {
        let names: Vec<(String, u32)> = (0..16).map(|i| (format!("NAME_{i:02}_ABCDEFGH"), 1)).collect();
        let flow = sender_flow(&names, Language::English);
        let out = flow_two_lines(&flow, &widths(&flow), 60.0);
        let shown = out.iter().map(|l| l.matches("NAME_").count()).sum::<usize>();
        let more: usize = out[1].rsplit('+').next().unwrap().parse().unwrap();
        assert_eq!(shown + more, 16);
        assert!(out[0].starts_with("Yo from NAME_00") && out[0].ends_with(','));
    }
}
