mod bits;
mod deflate;
mod gzip;
mod huffman;
mod inflate;

use inflate::BlockStats;
use std::io::{Read, Write};
use std::path::Path;
use std::process::exit;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const AIDE: &str = "\
degonfle — compresse et décompresse au format gzip (DEFLATE), écrit de zéro

  degonfle [options] [fichiers...]     sans fichier : entrée standard -> sortie standard

  -d          décompresser
  -c          écrire sur la sortie standard
  -1 ... -9   compromis vitesse / taille (défaut -6) ; -0 : ne pas compresser
  -f          écraser le fichier de sortie s'il existe
  -v          détails : taux, vitesse, types de blocs

Contrairement à gzip, le fichier d'origine n'est jamais supprimé.
";

struct Opts {
    decompress: bool,
    stdout: bool,
    level: u32,
    force: bool,
    verbose: bool,
}

fn fail(msg: &str) -> ! {
    eprintln!("degonfle : {msg}");
    exit(1)
}

fn human(n: usize) -> String {
    match n {
        0..1024 => format!("{n} o"),
        1024..1048576 => format!("{:.1} Ko", n as f64 / 1024.0),
        _ => format!("{:.1} Mo", n as f64 / 1048576.0),
    }
}

fn process(input: &[u8], name: Option<&str>, opt: &Opts) -> Vec<u8> {
    let mut stats = BlockStats::default();
    let t = Instant::now();
    let out = if opt.decompress {
        gzip::decompress(input, &mut stats).unwrap_or_else(|e| fail(&format!("{} : {e}", name.unwrap_or("entrée"))))
    } else {
        let mtime = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
        gzip::compress(input, name, mtime, opt.level, &mut stats)
    };
    if opt.verbose {
        let dt = t.elapsed().as_secs_f64();
        let (raw, packed) = if opt.decompress { (out.len(), input.len()) } else { (input.len(), out.len()) };
        eprintln!(
            "{} : {} -> {} ({:.1} %), {:.1} Mo/s, blocs : {} dynamiques, {} fixes, {} non compressés",
            name.unwrap_or("-"),
            human(raw),
            human(packed),
            if raw > 0 { 100.0 * packed as f64 / raw as f64 } else { 0.0 },
            raw as f64 / 1048576.0 / dt.max(1e-9),
            stats.dynamic,
            stats.fixed,
            stats.stored
        );
    }
    out
}

fn main() {
    let mut opt = Opts { decompress: false, stdout: false, level: 6, force: false, verbose: false };
    let mut files = Vec::new();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "-h" | "--aide" | "--help" => {
                print!("{AIDE}");
                return;
            }
            "--decompresser" => opt.decompress = true,
            "-" => files.push(a),
            // options groupées à la gzip : -dc, -9v...
            _ if a.starts_with('-') && !a.starts_with("--") => {
                for c in a[1..].chars() {
                    match c {
                        'd' => opt.decompress = true,
                        'c' => opt.stdout = true,
                        'f' => opt.force = true,
                        'v' => opt.verbose = true,
                        '0'..='9' => opt.level = c as u32 - '0' as u32,
                        _ => fail(&format!("option inconnue -{c}")),
                    }
                }
            }
            _ if a.starts_with("--") => fail(&format!("option inconnue {a}")),
            _ => files.push(a),
        }
    }

    if files.is_empty() || files == ["-"] {
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input).unwrap_or_else(|e| fail(&e.to_string()));
        let out = process(&input, None, &opt);
        std::io::stdout().write_all(&out).unwrap_or_else(|e| fail(&e.to_string()));
        return;
    }

    for f in &files {
        let input = std::fs::read(f).unwrap_or_else(|e| fail(&format!("{f} : {e}")));
        let base = Path::new(f).file_name().and_then(|n| n.to_str());
        let out = process(&input, if opt.decompress { Some(f) } else { base }, &opt);
        if opt.stdout {
            std::io::stdout().write_all(&out).unwrap_or_else(|e| fail(&e.to_string()));
            continue;
        }
        let target = if opt.decompress {
            match f.strip_suffix(".gz") {
                Some(t) if !t.is_empty() => t.to_string(),
                _ => fail(&format!("{f} : pas d'extension .gz, utilise -c")),
            }
        } else {
            format!("{f}.gz")
        };
        if Path::new(&target).exists() && !opt.force {
            fail(&format!("{target} existe déjà (-f pour écraser)"));
        }
        std::fs::write(&target, &out).unwrap_or_else(|e| fail(&format!("{target} : {e}")));
    }
}
