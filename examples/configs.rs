//! Prints /syncapi/v1/getconfigs for every module (read-only).
fn main() {
    let mut c = bticino_easykit::Client::new().unwrap();
    let h = c.homes().unwrap().into_iter().find(|h| h.bridge().is_some()).unwrap();
    let ids: Vec<&str> = h.modules.iter().map(|m| m.id.as_str()).collect();
    println!("{:#}", c.configs(&h.id, &ids).unwrap());
}
