fn main() {
    let root = std::env::args().nth(1).expect("server folder");
    let root = std::path::Path::new(&root);
    println!("{:?}", coa_core::population::query(root));
    let h = coa_core::population::hardware();
    println!("{h:?}");
    for s in coa_core::population::sizes(&h) {
        println!("{} {} {:?}", s.title, s.bots, s.warning);
    }
}
