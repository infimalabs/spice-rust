use std::ffi::OsString;

use super::*;

const TESTING: &str = "'/opt/ms-playwright/chromium-1248/chrome-mac-arm64/\
    Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing'";

#[test]
fn a_chromium_executable_in_command_position_needs_the_mock_keychain() {
    let refused = [
        format!(
            "{TESTING} --headless=new --disable-gpu --no-first-run \
             --remote-debugging-port=9225 --user-data-dir=/tmp/review about:blank"
        ),
        "cd _site && chromium --headless --dump-dom index.html | head".to_owned(),
        "PORT=9225 nohup google-chrome --headless &".to_owned(),
        "env DISPLAY=:0 exec /usr/bin/chromium-browser --headless".to_owned(),
        "(\"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome\" --headless)".to_owned(),
        "if true; then /opt/Google\\ Chrome\\ for\\ Testing --headless; fi".to_owned(),
        // The switch counts only on the browser's own simple command.
        "chromium --headless; echo --use-mock-keychain".to_owned(),
    ];
    for script in &refused {
        let refusal = refusal(script).unwrap_or_else(|| panic!("admitted: {script}"));
        assert!(refusal.contains("browser::keychain-unmocked"), "{refusal}");
    }

    let admitted = [
        format!("{TESTING} --headless=new --use-mock-keychain about:blank"),
        "chromium --use-mock-keychain --password-store=basic --headless".to_owned(),
        // Naming a browser as an argument or in text starts nothing.
        format!("ls -l {TESTING}"),
        "grep -rn chromium app/browser; echo 'Google Chrome'".to_owned(),
        "node app/browser/check.mjs".to_owned(),
        "echo \"chromium --headless\"".to_owned(),
    ];
    for script in &admitted {
        assert_eq!(refusal(script), None, "{script}");
    }
}

#[test]
fn openers_and_command_prefixes_lead_into_the_browser_they_start() {
    for script in [
        "if chromium --headless; then :; fi",
        "while chromium --headless; do :; done",
        "until chromium --headless; do :; done",
        "timeout 30 chromium --headless",
        "timeout -s KILL -k 5 30s chromium --headless",
        "sudo -u runner -E chromium --headless",
        "sudo -- chromium --headless",
    ] {
        assert!(refusal(script).is_some(), "admitted: {script}");
        let switched = script.replace("--headless", "--headless --use-mock-keychain");
        assert_eq!(refusal(&switched), None, "{switched}");
    }
    // An option's value or a duration names no command.
    for script in ["sudo -u chromium ls", "timeout chromium make test"] {
        assert_eq!(refusal(script), None, "{script}");
    }
}

#[test]
fn heredoc_bodies_are_data_but_their_surroundings_are_commands() {
    let admitted = [
        "git commit -F - <<'MSG'\nchromium --headless no longer prompts\nMSG",
        "cat > notes.md <<EOF\nchromium --headless\nEOF\necho done",
        "cat <<-\"END\"\n\tchromium --headless\n\tEND\n",
        "cat <<A <<B\nchromium --headless\nA\ngoogle-chrome --headless\nB",
        "cat <<EOF\nchromium --headless\n",
    ];
    for script in admitted {
        assert_eq!(refusal(script), None, "{script}");
    }
    let refused = [
        "cat <<EOF; chromium --headless\nbody\nEOF",
        "cat <<EOF\nbody\nEOF\nchromium --headless",
        "cat <<-END\n\tbody\n\tEND\nchromium --headless",
        // A here-string, quoted text, and a comment open no body.
        "cat <<<'x'\nchromium --headless",
        "echo '<<EOF'\nchromium --headless",
        "echo hi # a<<EOF\nchromium --headless",
    ];
    for script in refused {
        assert!(refusal(script).is_some(), "admitted: {script}");
    }
}

#[test]
fn a_wrapped_vector_is_judged_by_its_script_or_as_one_command() {
    let words = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    assert!(argv_refusal(&words(&["/bin/zsh", "-c", "chromium --headless"])).is_some());
    assert!(argv_refusal(&words(&["/bin/zsh", "-c", "echo chromium"])).is_none());
    assert!(argv_refusal(&words(&["/usr/bin/chromium", "--headless"])).is_some());
    assert!(argv_refusal(&words(&["/usr/bin/chromium", "--headless", MOCK_KEYCHAIN])).is_none());
    assert!(argv_refusal(&words(&["git", "status"])).is_none());
}
