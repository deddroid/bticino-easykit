//! Registers (or loads) the FCM identity only; touches nothing on the
//! Netatmo account. Useful to debug Google-side registration.
fn main() {
    match bticino_easykit::push::load_or_register() {
        Ok((_, st, new)) => println!(
            "{} android_id={} token_len={}",
            if new { "registered" } else { "loaded" },
            st.android_id,
            st.fcm_token.len()
        ),
        Err(e) => eprintln!("error: {e}"),
    }
}
