use std::{env::args, process, time::Duration};

use solarman_tokio::Client;
use tokio::time::sleep;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt::init();

    let mut args = args().skip(1);
    let (Some(addr), Some(serial), Some(slave_id)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: <ip> <stick serial> <slave id>");
        process::exit(1);
    };

    let mut client = Client::connect(addr, serial.parse().unwrap(), slave_id.parse().unwrap())
        .await
        .unwrap();

    for addr in 0..50000 {
        eprintln!("reading addr {addr}");
        match client.read_holding_registers(addr, 1).await {
            Ok(o) => eprintln!("success => {}", o[0]),
            Err(e) => eprintln!("error => {e:?}"),
        }
        sleep(Duration::from_millis(10000)).await;
    }

    client.shutdown().await.unwrap();
}
