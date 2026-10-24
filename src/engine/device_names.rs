//! Finding a MIDI device again by name, after the system has renamed it
//! (#436).
//!
//! Soba keeps devices by name: a MIDI Out port's device, the inputs to
//! open, a module's Device, a mapping's. But the names aren't steady.
//! Windows puts a number in front of a device that shares a name with
//! another, which depends on the order they were plugged in ("2- Digitakt"),
//! numbers the extra ports of a device ("MIDIIN3 (Fantom-06)"), and Linux
//! ends each with its client and port ("Digitakt:Digitakt MIDI 1 24:0"). A
//! reboot or a replug can change any of them.
//!
//! So a name is looked for exactly first, then by its [`key`]: the name
//! without those numbers. A key that two devices share finds neither,
//! rather than guess.

/// `name` without the numbers the system gives it, lower case: what's left
/// of a device's name when it's plugged in a different order.
pub fn key(name: &str) -> String {
    let mut name = name.trim();
    // Linux's client:port, at the end
    if let Some((head, tail)) = name.rsplit_once(' ') {
        if tail.split_once(':').is_some_and(|(client, port)| is_number(client) && is_number(port)) {
            name = head;
        }
    }
    let lower = name.to_lowercase();
    let mut key = String::with_capacity(lower.len());
    let mut rest = lower.as_str();
    // At the start, and at the start of what's in brackets
    let mut word_start = true;
    while !rest.is_empty() {
        if word_start {
            // "2- Digitakt"
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if digits > 0 && rest[digits..].starts_with("- ") {
                rest = &rest[digits + 2..];
                continue;
            }
            // "MIDIIN3 (Fantom-06)", "MIDIOUT2 (Fantom-06)": the port's word stays
            for word in ["midiin", "midiout"] {
                if let Some(after) = rest.strip_prefix(word) {
                    let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                    if digits > 0 {
                        key.push_str(word);
                        rest = &after[digits..];
                    }
                }
            }
        }
        let Some(c) = rest.chars().next() else { break };
        key.push(c);
        rest = &rest[c.len_utf8()..];
        word_start = c == '(';
    }
    key
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_digit())
}

/// Are `a` and `b` the same device, as far as their names say?
pub fn same(a: &str, b: &str) -> bool {
    a == b || key(a) == key(b)
}

/// Are the MIDI input called `input` and the output called `output` the
/// same device (#438)? Most devices give both the same name; Windows calls
/// a device's numbered ports "MIDIIN2 (...)" one way and "MIDIOUT2 (...)"
/// the other.
pub fn in_and_out(input: &str, output: &str) -> bool {
    !input.is_empty() && !output.is_empty() && (same(input, output) || same(&input.to_lowercase().replace("midiin", "midiout"), output))
}

/// The device of those `found` that `wanted` names: the one called that,
/// or else the only one whose [`key`] is the same.
pub fn find<'a>(wanted: &str, found: &'a [String]) -> Option<&'a str> {
    if let Some(exact) = found.iter().find(|name| *name == wanted) {
        return Some(exact);
    }
    let wanted = key(wanted);
    let mut alike = found.iter().filter(|name| key(name) == wanted);
    match (alike.next(), alike.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// The device of those `found` that each of `wanted` names, as [`find`]
/// would, except that a device another name finds exactly isn't found for
/// a near one. Two Digitakts were "Digitakt" and "2- Digitakt", and after a
/// reboot are "Digitakt" and "3- Digitakt": the first keeps its own, and
/// the second has the one that's left.
pub fn find_all<'a>(wanted: &[String], found: &'a [String]) -> Vec<Option<&'a str>> {
    let exact: Vec<&str> = wanted.iter().filter_map(|name| found.iter().find(|f| *f == name)).map(String::as_str).collect();
    let free: Vec<String> = found.iter().filter(|name| !exact.contains(&name.as_str())).cloned().collect();
    wanted
        .iter()
        .map(|name| {
            if name.is_empty() {
                return None;
            }
            if let Some(own) = found.iter().find(|f| *f == name) {
                return Some(own.as_str());
            }
            let near = find(name, &free)?;
            found.iter().find(|f| *f == near).map(String::as_str)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn keys_drop_the_systems_numbers() {
        assert_eq!(key("2- Digitakt"), "digitakt");
        assert_eq!(key("Digitakt"), "digitakt");
        assert_eq!(key("MIDIIN3 (Fantom-06)"), "midiin (fantom-06)");
        assert_eq!(key("MIDIIN2 (2- FANTOM-06)"), "midiin (fantom-06)");
        assert_eq!(key("MIDIOUT2 (Fantom-06)"), "midiout (fantom-06)");
        assert_eq!(key("Digitakt:Digitakt MIDI 1 24:0"), "digitakt:digitakt midi 1");
        // Numbers that are the device's own stay
        assert_eq!(key("volca fm2"), "volca fm2");
        assert_eq!(key("Fantom-06"), "fantom-06");
        assert_eq!(key("MIDIIN (Zoia)"), "midiin (zoia)");
        assert_eq!(key("12 Step"), "12 step");
    }

    #[test]
    fn a_renumbered_device_is_found_again() {
        let found = names(&["3- Digitakt", "MIDIIN4 (FANTOM-06)", "MicroFreak"]);
        assert_eq!(find("2- Digitakt", &found), Some("3- Digitakt"));
        assert_eq!(find("Digitakt", &found), Some("3- Digitakt"));
        assert_eq!(find("MIDIIN3 (Fantom-06)", &found), Some("MIDIIN4 (FANTOM-06)"));
        assert_eq!(find("MicroFreak", &found), Some("MicroFreak"));
        assert_eq!(find("Fantom-06", &found), None, "a device's first port isn't its third");
        assert_eq!(find("SP-404MKII", &found), None);
        assert!(same("2- Digitakt", "Digitakt"));
        assert!(!same("Digitakt", "Digitakt II"));
    }

    #[test]
    fn a_devices_input_and_output_are_told_to_be_one() {
        assert!(in_and_out("Digitakt", "Digitakt"));
        assert!(in_and_out("2- Digitakt", "Digitakt"));
        assert!(in_and_out("MIDIIN2 (FANTOM-06)", "MIDIOUT2 (FANTOM-06)"));
        assert!(in_and_out("FANTOM-06", "FANTOM-06"));
        assert!(!in_and_out("FANTOM-06", "MIDIOUT2 (FANTOM-06)"), "the main port isn't the second");
        assert!(!in_and_out("Digitakt", "Digitakt II"));
        assert!(!in_and_out("Digitakt", ""), "a port with no device is no one's");
    }

    #[test]
    fn two_alike_find_neither_unless_one_is_exact() {
        let found = names(&["Digitakt", "3- Digitakt"]);
        assert_eq!(find("2- Digitakt", &found), None, "which one is a guess");
        assert_eq!(find("Digitakt", &found), Some("Digitakt"));
        let wanted = names(&["Digitakt", "2- Digitakt", "", "Gone"]);
        assert_eq!(find_all(&wanted, &found), [Some("Digitakt"), Some("3- Digitakt"), None, None]);
    }
}
