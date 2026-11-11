//! What went wrong with a join, in words a player understands (DCM-70).
//!
//! A join can fail at any step: the server isn't there, it speaks another protocol, it refuses the
//! hello (password, full, name, ban, the same key twice), its mods can't be had, or this game's
//! data builds another town. Each step says why in its own words, written for the log. The join
//! screen shows a [`Problem`] instead: a short title and a sentence or two on what to do, and for
//! another version of the game, which version each side is ([`crate::BUILD`] here, the server's
//! from its answer to a query).

/// A failed join, for the join screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// A few words: "Wrong password", "Different version".
    pub title: String,
    /// What it means and what to do, in full sentences.
    pub text: String,
    /// Whether trying again as it is could work (a server that wasn't there may be up again; a
    /// wrong password stays wrong). The screen offers Retry either way.
    pub retry_helps: bool,
}

/// Who the join was with, for the words: the server's name (or its address) and, once it has
/// answered a query, the version it says it is.
#[derive(Clone, Debug, Default)]
pub struct Server<'a> {
    pub name: &'a str,
    pub addr: &'a str,
    pub version: Option<&'a str>,
}

impl Server<'_> {
    fn called(&self) -> &str {
        if self.name.trim().is_empty() { self.addr } else { self.name }
    }
}

/// `raw` (the error a join step gave) as a [`Problem`].
pub fn explain(raw: &str, server: &Server) -> Problem {
    let low = raw.to_lowercase();
    let who = server.called();
    let p = |title: &str, text: String, retry_helps: bool| Problem { title: title.into(), text, retry_helps };
    let has = |s: &str| low.contains(s);
    // Another build of the game: another protocol, other game data, another town from the seed,
    // or a hello the server can't read.
    if has("speaks protocol") || has("runs version") || has("game data isn't the server's") || has("different town") || has("couldn't prove who you are") {
        let what = raw.find("(").and_then(|a| raw[a + 1..].find(" differ)").map(|b| raw[a + 1..a + 1 + b].to_string())).filter(|_| has("game data"));
        let versions = match server.version {
            Some(theirs) if !theirs.trim().is_empty() => format!("{who} runs Zombye {theirs}, and your game is {}.", crate::BUILD),
            _ => format!("Your game is Zombye {}.", crate::BUILD),
        };
        let differs = what.map_or_else(String::new, |w| format!(" (Their {w} differ from yours.)"));
        return p("Different version", format!("{versions}{differs} Both have to be built from the same version of the game: update whichever is behind and try again."), false);
    }
    if has("wrong password") {
        return p("Wrong password", format!("{who} needs a password, and that one isn't it. Check it with whoever runs the server."), false);
    }
    if has("needs a password") {
        return p("Password needed", format!("{who} needs a password. Go back, type it in the server list's Password field and join again."), false);
    }
    if has("is full") {
        return p("Server full", format!("{who} has no room for another player right now. Try again in a while."), true);
    }
    if has("same player key") {
        return p("Already playing", format!("This game is already on {who}, from another window or computer that uses the same saves folder. Quit that one first."), true);
    }
    if has("is already playing here") {
        let name = raw.split(" is already playing here").next().unwrap_or_default().rsplit(": ").next().unwrap_or_default().trim();
        let name = if name.is_empty() { "That name".to_string() } else { format!("Someone called {name}") };
        return p("Name taken", format!("{name} is already on {who}. Join under another name."), false);
    }
    if has("banned") {
        let why = raw.split_once("banned from this server: ").map(|(_, w)| format!(" The reason given: {w}.")).unwrap_or_default();
        return p("Banned", format!("{who} doesn't let you in.{why}"), false);
    }
    if has("first person") {
        return p("First person only", format!("{who} only allows a first-person camera, and this version of the game doesn't have one yet."), false);
    }
    if has("don't allow agents") {
        return p("No agents", format!("{who} doesn't allow games played by a program, and this one says it is (--agent or --autoplay)."), false);
    }
    if has("too big to download") || (has("mod") && has("to download")) {
        return p("Mods too big", format!("{who}'s mods are too big to download on joining. Get them from whoever runs it and start the game with --mod."), false);
    }
    if has("mods don't fit") || has("mod ") && has("the server") && !has("connection") {
        return p("Mods don't fit", format!("{who}'s mods don't fit this version of the game: {raw}."), false);
    }
    if has("player key") {
        return p("Player key", format!("Your game's player key can't be used: {raw}. It is identity.key in the saves folder."), false);
    }
    if has("can't find") || has("failed to lookup") || has("name or service not known") || has("nodename nor servname") {
        return p("Server not found", format!("There's no server at the address {}. Check it for typing mistakes.", server.addr), false);
    }
    if has("refused") && !has("refused:") || has("timed out") || has("os error 111") || has("os error 61") || has("os error 10061") || has("unreachable") || has("can't connect") {
        return p("Can't reach the server", format!("Nothing answered at {}. The server may be off, or a firewall or the network is in the way.", server.addr), true);
    }
    if has("lost the connection") || has("connection reset") || has("broken pipe") || has("closed the connection") || has("disconnected") {
        return p("Connection lost", format!("The connection to {who} dropped while joining. Check the network and try again."), true);
    }
    let mut text = raw.trim().to_string();
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    if !text.ends_with('.') {
        text.push('.');
    }
    p("Can't join", text, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at<'a>(version: Option<&'a str>) -> Server<'a> {
        Server { name: "Dead Commons", addr: "play.example:7777", version }
    }

    #[test]
    fn every_refusal_reads_as_a_sentence() {
        let cases = [
            ("this server speaks protocol 22, your game 21: both need the same version of the game", "Different version"),
            ("your game data isn't the server's (weapons, items differ): both need the same version of the game and its data folder", "Different version"),
            ("your game builds a different town from the server's seed: both need the same version of the game", "Different version"),
            ("your game couldn't prove who you are (its player key): both need the same version of the game", "Different version"),
            ("refused: wrong password", "Wrong password"),
            ("refused: the server is full (32 players)", "Server full"),
            ("refused: this game is already playing here (the same player key)", "Already playing"),
            ("refused: ada is already playing here", "Name taken"),
            ("refused: you are banned from this server: griefing", "Banned"),
            ("Dead Commons is first person only, and this version of the game has no first-person camera yet", "First person only"),
            ("refused: this server's rules don't allow agents", "No agents"),
            ("can't find nowhere.invalid:7777: failed to lookup address information", "Server not found"),
            ("can't connect to 127.0.0.1:7777: Connection refused (os error 111)", "Can't reach the server"),
            ("lost the connection to 127.0.0.1:7777: connection reset by peer", "Connection lost"),
        ];
        for (raw, title) in cases {
            let p = explain(raw, &at(Some("0.1.0 (abc1234)")));
            assert_eq!(p.title, title, "{raw}");
            assert!(p.text.ends_with('.') && p.text.chars().next().unwrap().is_uppercase(), "{raw}: {}", p.text);
        }
    }

    #[test]
    fn another_version_names_both_and_what_differs() {
        let p = explain("your game data isn't the server's (weapons differ): both need the same version", &at(Some("0.1.0 (abc1234)")));
        assert!(p.text.contains("Dead Commons runs Zombye 0.1.0 (abc1234)"), "{}", p.text);
        assert!(p.text.contains(&format!("your game is {}", crate::BUILD)), "{}", p.text);
        assert!(p.text.contains("Their weapons differ"), "{}", p.text);
        let p = explain("this server speaks protocol 22, your game 21", &at(None));
        assert!(p.text.starts_with(&format!("Your game is Zombye {}", crate::BUILD)), "{}", p.text);
        // Names and the ban's reason come through; anything else is the raw words, tidied.
        assert!(explain("refused: ada is already playing here", &at(None)).text.starts_with("Someone called ada is already on Dead Commons"));
        assert!(explain("refused: you are banned from this server: griefing", &at(None)).text.contains("griefing"));
        assert_eq!(explain("something odd", &at(None)), Problem { title: "Can't join".into(), text: "Something odd.".into(), retry_helps: true });
    }
}
