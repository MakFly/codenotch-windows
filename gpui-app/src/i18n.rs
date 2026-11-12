pub fn t(french: bool, english: &str, french_text: &str) -> String {
    if french {
        french_text.to_string()
    } else {
        english.to_string()
    }
}
