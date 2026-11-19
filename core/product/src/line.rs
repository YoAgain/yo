//! A group bar's Yos from two or more senders: every name, newest first, flowing over two small lines
//! ("Yo from TOM, IDA, BEN (3×)," / "SAM, KAI +4"). The platform measures the pieces at the small size and
//! renders the two lines; the core decides what goes where. Names are never cut (usernames hold at most 16
//! graphemes and always fit at the small size); whatever does not fit ends the second line as " +N".
use crate::{counted, Language};

/// The pieces a platform measures, in the bar's small font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flow {
    /// "Yo from " (with its space), "Yo von ".
    pub prefix: String,
    /// One per sender, newest first: "TOM", "BEN (3×)".
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

/// One sender's Yos on the bar, newest first: name and count.
pub fn sender_flow(senders: &[(String, u32)], lang: Language) -> Flow {
    let yo = counted(1);
    let from = lang.choose("from", "von");
    Flow {
        prefix: format!("{yo} {from} "),
        tokens: senders
            .iter()
            .map(|(name, count)| if *count > 1 { format!("{name} ({count}×)") } else { name.clone() })
            .collect(),
        separator: ", ".into(),
        comma: ",".into(),
        more: (1..senders.len()).map(|n| format!(" +{n}")).collect(),
    }
}

/// The two lines (the second may be empty) for the room the bar has. Widths add up (kerning between pieces is
/// within the room's margin); a name that does not fit even alone on the first line is still placed (never cut).
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
        if placed > 0 && used + add + comma > room {
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
    if placed < n {
        first.push_str(&flow.comma);
        let mut used = 0.0;
        let start = placed;
        while placed < n {
            let add = if placed == start { 0.0 } else { widths.separator } + width(placed);
            if used + add + more(n - placed - 1) > room {
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
            let tail = &flow.more[n - placed - 1];
            second.push_str(if second.is_empty() { tail.trim_start() } else { tail });
        }
    }
    [first, second]
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
    fn lines(list: &[(&str, u32)], room: f32) -> [String; 2] {
        let flow = sender_flow(&senders(list), Language::English);
        let widths = widths(&flow);
        let out = flow_two_lines(&flow, &widths, room);
        for line in &out {
            assert!(line.chars().count() as f32 <= room || line.starts_with("Yo from"), "{line:?} over {room}");
        }
        out
    }

    #[test]
    fn pieces_count_only_above_one_and_speak_german() {
        let flow = sender_flow(&senders(&[("TOM", 1), ("BEN", 3), ("IDA", 1)]), Language::English);
        assert_eq!(flow.prefix, "Yo from ");
        assert_eq!(flow.tokens, ["TOM", "BEN (3×)", "IDA"]);
        assert_eq!(flow.more, [" +1", " +2"]);
        assert_eq!(sender_flow(&senders(&[("MIA", 1)]), Language::German).prefix, "Yo von ");
    }

    #[test]
    fn all_fit_on_the_first_line() {
        assert_eq!(lines(&[("TOM", 1), ("IDA", 2)], 40.0), ["Yo from TOM, IDA (2×)", ""]);
    }

    #[test]
    fn the_first_line_ends_with_a_comma_and_the_second_continues() {
        // "Yo from TOM, IDA, BEN (3×)," is 27 wide.
        let out = lines(&[("TOM", 1), ("IDA", 1), ("BEN", 3), ("SAM", 1), ("KAI", 1)], 27.0);
        assert_eq!(out, ["Yo from TOM, IDA, BEN (3×),", "SAM, KAI"]);
    }

    #[test]
    fn whatever_does_not_fit_ends_the_second_line_as_more() {
        let names: Vec<_> = ["TOM", "IDA", "BEN", "SAM", "KAI", "EVA", "MAX", "ANA", "LOU"].map(|n| (n, 1)).to_vec();
        // Line 1: "Yo from TOM, IDA, BEN," (22). Line 2: "SAM, KAI, EVA, MAX +2" (21); ANA would need 23.
        assert_eq!(lines(&names, 22.0), ["Yo from TOM, IDA, BEN,", "SAM, KAI, EVA, MAX +2"]);
    }

    #[test]
    fn the_last_name_needs_no_more() {
        // "SAM, KAI" (8) fits exactly; with " +1" it would need 11, but nothing is left after KAI.
        let out = lines(&[("TOM", 1), ("IDA", 1), ("BEN", 1), ("SAM", 1), ("KAI", 1)], 22.0);
        assert_eq!(out, ["Yo from TOM, IDA, BEN,", "SAM, KAI"]);
    }

    #[test]
    fn a_second_line_without_room_for_a_name_is_only_more() {
        let flow = sender_flow(&senders(&[("A", 1), ("LONGNAMEXX", 1), ("C", 1)]), Language::English);
        let mut widths = widths(&flow);
        widths.tokens[1] = 100.0;
        assert_eq!(flow_two_lines(&flow, &widths, 12.0), ["Yo from A,", "+2"]);
    }

    #[test]
    fn a_name_too_wide_alone_is_placed_whole_never_cut() {
        let flow = sender_flow(&senders(&[("MAXIMILIAN_MUELL", 1), ("IDA", 1)]), Language::English);
        let out = flow_two_lines(&flow, &widths(&flow), 10.0);
        assert_eq!(out, ["Yo from MAXIMILIAN_MUELL,", "IDA"]);
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
