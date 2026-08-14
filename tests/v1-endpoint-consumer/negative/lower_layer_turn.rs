use sipx_media::ice::Relay;

fn main() {
    let _ = Relay::new("192.0.2.20:3478".parse().unwrap(), "1000", "secret");
}
