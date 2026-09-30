fn main() {
    let root = std::env::args().nth(1).expect("server folder");
    let ports = coa_core::layout::read_ports(std::path::Path::new(&root));
    println!("LAN address: {:?}", coa_core::net::lan_ip());
    for e in coa_core::net::exposure(&ports) {
        println!("{:<14} port {:<5} listening={} reachable_from_network={}", e.what, e.port, e.listening, e.reachable_from_network);
    }
    println!("{:?}", coa_core::net::tailscale());
    println!("firewall rules: {:?}", coa_core::firewall::status());
    match coa_core::upnp::discover() {
        Some(gw) => println!("UPnP router: {} -> outside address {:?}", gw.control_url, coa_core::upnp::external_ip(&gw)),
        None => println!("UPnP router: none answered"),
    }
}
