//! keeps a note on the memory card that says "we have a cloudmatch session open somewhere".
//! needed bc if the app crashes/force-quits the normal stop never runs and the account gets
//! locked out for a few min until nvidia reaps it. same idea as OpenNOW-Switch's cloud_session_state.cpp

const STORE_DIR: &str = "ux0:data/opennow-vita";
const STORE_PATH: &str = "ux0:data/opennow-vita/active-session.txt";

// session we opened but havent confirmed closed yet
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleSession {
    pub session_id: String,
    // zone that opened it, empty if we never recorded it (falls back to default entrypoint)
    pub streaming_base_url: String,
}

pub fn remember(session_id: &str, streaming_base_url: &str) {
    if session_id.is_empty() || std::fs::create_dir_all(STORE_DIR).is_err() {
        return;
    }
    // tab separated, urls dont have whitespace and session ids are uuids so no ambiguity
    let line = format!("{session_id}\t{streaming_base_url}");
    if let Err(error) = std::fs::write(STORE_PATH, line) {
        eprintln!("Could not record the active CloudMatch session: {error}");
    }
}

// takes the id so a stop racing a newer launch doesnt wipe the newer session's note
pub fn forget(session_id: &str) {
    match load() {
        Some(stale) if stale.session_id == session_id => {}
        Some(_) => return,
        None => return,
    }
    let _ = std::fs::remove_file(STORE_PATH);
}

pub fn load() -> Option<StaleSession> {
    let contents = std::fs::read_to_string(STORE_PATH).ok()?;
    let line = contents.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let (session_id, streaming_base_url) = match line.split_once('\t') {
        Some((id, url)) => (id.trim(), url.trim()),
        // old note from before we started recording the zone url
        None => (line, ""),
    };
    if session_id.is_empty() {
        return None;
    }
    Some(StaleSession {
        session_id: session_id.to_owned(),
        streaming_base_url: streaming_base_url.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_note_with_a_zone_url() {
        let line = "abc-123\thttps://zone.example/";
        let (id, url) = line.split_once('\t').expect("line should split");
        assert_eq!(id, "abc-123");
        assert_eq!(url, "https://zone.example/");
    }

    #[test]
    fn a_note_without_a_zone_url_still_names_its_session() {
        assert!("abc-123".split_once('\t').is_none());
    }
}
