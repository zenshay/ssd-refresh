//! Synthetic CPU checks for compiler experiments. No storage volume operations.
use sha2::{Digest, Sha256};
use std::hint::black_box;
use std::time::{Duration, Instant};

fn rate(mut work: impl FnMut()) -> u128 {
    let start = Instant::now();
    let mut count = 0u128;
    while start.elapsed() < Duration::from_millis(700) {
        work();
        count += 1;
    }
    start.elapsed().as_nanos() / count
}

fn main() {
    let data = vec![0x5au8; 8 * 1024 * 1024];
    let expected = Sha256::digest(&data);
    let sha_ns = rate(|| {
        let result = Sha256::digest(black_box(&data));
        assert_eq!(result, expected);
        black_box(result);
    });
    let folder = std::env::temp_dir().join(format!("ssd-compile-probe-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("synthetic.csv");
    let mut csv = String::from("id,path,value\n");
    for i in 0..10000 {
        csv.push_str(&format!("{i},\"synthetic,{i}\",{i}\n"));
    }
    std::fs::write(&path, csv).unwrap();
    let csv_ns = rate(|| {
        let rows = ssd_refresh::csv::read_all(black_box(&path)).unwrap();
        assert_eq!(rows.len(), 10000);
        assert_eq!(rows[9999]["path"], "synthetic,9999");
        black_box(rows);
    });
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&folder).unwrap();
    let digest: String = expected.iter().map(|b| format!("{b:02x}")).collect();
    println!(
        "{{\"sha_ns\":{sha_ns},\"csv_ns\":{csv_ns},\"sha_digest\":\"{digest}\",\"rows\":10000}}"
    );
}
