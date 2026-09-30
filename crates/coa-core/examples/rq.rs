use std::io::Read;
fn main() {
    let url = std::env::args().nth(1).unwrap();
    let mode = std::env::args().nth(2).unwrap();
    let t = std::time::Instant::now();
    let client = reqwest::blocking::Client::builder().timeout(None::<std::time::Duration>).build().unwrap();
    let mut resp = client.get(&url).send().unwrap();
    if mode == "bytes" {
        let b = resp.bytes().unwrap();
        println!("bytes {} in {:?}", b.len(), t.elapsed());
    } else {
        let mut buf = vec![0u8; 256 * 1024];
        let mut total = 0;
        loop {
            let n = resp.read(&mut buf).unwrap();
            if n == 0 { break; }
            total += n;
        }
        println!("read loop {} in {:?}", total, t.elapsed());
    }
}
