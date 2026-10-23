fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let job = coa_core::download::Job {
        url: a[0].clone(),
        dest: a[1].clone().into(),
        sha256: a[2].clone(),
        size: a[3].parse().unwrap(),
    };
    let t = std::time::Instant::now();
    let r = coa_core::download::fetch(&job, &Default::default(), &|p| {
        eprintln!("{}/{}", p.downloaded, p.total)
    });
    println!("{r:?} in {:?}", t.elapsed());
}
