//! Rules for a new password the server itself accepts.
//!
//! Existing accounts are not rejected: a sign-in with an older password still
//! works, and the caller can warn. PJDesk Cloud sign-in is not checked here.
//! That password is verified by pjdesk.com and never stored on this server.

/// Shortest password this server will store for a new or changed account.
pub const MIN_LEN: usize = 12;

/// One line of the live checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Check {
    pub label: &'static str,
    pub met: bool,
}

/// Result of checking a candidate password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub ok: bool,
    /// 0 empty, 1 incomplete, 2 acceptable, 3 strong, 4 very strong.
    pub score: u8,
    pub checks: Vec<Check>,
    pub errors: Vec<String>,
}

/// Checks `password` against `identity` (a username or an email).
pub fn evaluate(password: &str, identity: &str) -> Report {
    let trimmed_changed = password != password.trim();
    let len_ok = password.chars().count() >= MIN_LEN;
    let lower = password.chars().any(|c| c.is_ascii_lowercase());
    let upper = password.chars().any(|c| c.is_ascii_uppercase());
    let digit = password.chars().any(|c| c.is_ascii_digit());
    let symbol = password.chars().any(|c| !c.is_ascii_alphanumeric() && !c.is_whitespace());
    let identity_ok = !contains_identity(password, identity);
    let common = is_common(password);
    let not_common = !common;

    let checks = vec![
        Check { label: "At least 12 characters", met: len_ok && !trimmed_changed },
        Check { label: "One uppercase letter", met: upper },
        Check { label: "One lowercase letter", met: lower },
        Check { label: "One digit", met: digit },
        Check { label: "One symbol", met: symbol },
        Check { label: "Does not contain the username", met: identity_ok },
        Check { label: "Not a common password", met: not_common },
        Check { label: "No leading or trailing space", met: !trimmed_changed },
    ];

    let mut errors = Vec::new();
    if trimmed_changed {
        errors.push("Password must not start or end with a space.".into());
    }
    if !len_ok {
        errors.push(format!("Password must be at least {MIN_LEN} characters."));
    }
    if !upper || !lower || !digit || !symbol {
        errors.push("Use an uppercase letter, a lowercase letter, a digit, and a symbol.".into());
    }
    if !identity_ok {
        errors.push("Password must not contain the username or the email name.".into());
    }
    if common {
        errors.push("That password is too common. Choose one that is not on the known-breached list.".into());
    }

    let classes = [lower, upper, digit, symbol].into_iter().filter(|c| *c).count();
    let score = if password.is_empty() {
        0
    } else if !errors.is_empty() {
        1
    } else if password.chars().count() >= 20 && classes == 4 {
        4
    } else if password.chars().count() >= 16 {
        3
    } else {
        2
    };

    Report { ok: errors.is_empty(), score, checks, errors }
}

/// `Err` with the checklist failures joined into one sentence.
pub fn require(password: &str, identity: &str) -> anyhow::Result<()> {
    let report = evaluate(password, identity);
    if report.ok {
        Ok(())
    } else {
        anyhow::bail!("{}", report.errors.join(" "));
    }
}

/// True when an existing password would be refused if it were set today.
pub fn is_weak(password: &str, identity: &str) -> bool {
    !evaluate(password, identity).ok
}

/// Sentence shown after a successful sign-in with an older weak password.
pub fn weak_login_message() -> &'static str {
    "This password is weaker than the current policy. It still works, but change it to at least 12 characters with upper and lower case, a digit, and a symbol."
}

fn contains_identity(password: &str, identity: &str) -> bool {
    let id = identity.trim();
    if id.is_empty() {
        return false;
    }
    let local = id.split('@').next().unwrap_or(id);
    let local = local.trim().to_ascii_lowercase();
    if local.chars().count() < 3 {
        return false;
    }
    let pw = password.to_ascii_lowercase();
    pw.contains(&local)
}

fn is_common(password: &str) -> bool {
    let folded = fold(password);
    if folded.len() < 4 {
        return false;
    }
    COMMON.lines().any(|word| {
        let word = word.trim();
        // Short entries only reject an exact match, so a long passphrase is
        // not refused just because it contains "pass" or "root".
        !word.is_empty() && (folded == word || (word.len() >= 6 && folded.contains(word)))
    })
}

/// Lowercase, drop spaces, and fold a few look-alike characters so
/// `P@ssw0rd` still matches `password`.
fn fold(password: &str) -> String {
    password
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c.to_ascii_lowercase() {
            '0' => 'o',
            '1' | '!' => 'i',
            '3' => 'e',
            '4' | '@' => 'a',
            '$' | '5' => 's',
            '7' => 't',
            other => other,
        })
        .collect()
}

/// Compact list of common and breached passwords. Matching is on the folded
/// form, so this list itself is lowercase and already folded.
const COMMON: &str = "\
password
passwordi
passwordi!
passwordii
passwordiii
password123
password1234
passw0rd
p@ssw0rd
p@ssword
123456
1234567
12345678
123456789
1234567890
123123
123qwe
111111
000000
121212
654321
666666
696969
7777777
888888
999999
qwerty
qwerty123
qwertyuiop
qwertyi
1q2w3e4r
1q2w3e
1qaz2wsx
qazwsx
asdfgh
asdfghjkl
zxcvbn
abc123
letmein
letmeini
welcome
welcomei
admin
admini
admin123
root
toor
pass
guest
changeme
changemei
default
secret
hunter
hunter2
monkey
dragon
master
login
princess
princessi
football
baseball
soccer
shadow
sunshine
sunshinei
trustno1
iloveyou
iloveyoui
batman
access
hello
charlie
donald
whatever
freedom
nothing
starwars
superman
michael
jessica
computer
internet
photoshop
adobe123
million2
killer
zaq12wsx
pass123
root123
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_long_mixed_password() {
        let r = evaluate("Correct-horse-battery-9!", "ada");
        assert!(r.ok, "{:?}", r.errors);
        assert!(r.score >= 3);
        assert!(r.checks.iter().all(|c| c.met));
    }

    #[test]
    fn rejects_each_missing_rule() {
        assert!(!evaluate("short", "ada").ok);
        assert!(!evaluate("alllowercase1!", "ada").ok);
        assert!(!evaluate("ALLUPPERCASE1!", "ada").ok);
        assert!(!evaluate("NoDigitsHere!!", "ada").ok);
        assert!(!evaluate("NoSymbolHere12", "ada").ok);
        assert!(!evaluate("  Correct-horse-9!", "ada").ok);
        assert!(!evaluate("Correct-horse-9!  ", "ada").ok);
        assert!(evaluate("Correct-horse-9!", "ada").errors.iter().any(|e| e.contains("username")) == false);
        let named = evaluate("Correct-ada-9!", "Ada@example.com");
        assert!(!named.ok);
        assert!(named.errors.iter().any(|e| e.contains("username")));
    }

    #[test]
    fn rejects_common_and_leet_variants() {
        assert!(is_common("password"));
        assert!(is_common("Password1!"));
        assert!(is_common("p@ssw0rd"));
        assert!(is_common("hunter2"));
        assert!(!is_common("Correct-horse-battery-9!"));
        assert!(!evaluate("Password1!extra", "ada").ok);
    }

    #[test]
    fn short_identity_is_not_a_substring_trap() {
        let r = evaluate("Correct-horse-9!", "ab");
        assert!(r.ok, "{:?}", r.errors);
    }

    #[test]
    fn weak_login_is_a_warning_not_a_format_change() {
        assert!(is_weak("password", "ada"));
        assert!(!is_weak("Correct-horse-battery-9!", "ada"));
        assert!(weak_login_message().contains("still works"));
    }
}
