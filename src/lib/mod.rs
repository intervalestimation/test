
extern crate lazy_static;
extern crate log;
extern crate env_logger;
extern crate rust_htslib;
extern crate gcollections;
extern crate interval;

extern crate getopts;
extern crate regex;

use getopts::Options;
use std::env;
use std::cmp::Ord;
use std::path::Path;
use regex::Regex;

// use interval::Interval;
use std::thread;
use interval::interval_set::ToIntervalSet;
use interval::ops::*;
use gcollections::ops::*;

// use std::collections::BTreeMap;
use rust_htslib::bcf;
use std::cmp::Ordering;
use rust_htslib::bcf::Writer;
use std::collections::BinaryHeap;

struct Vcf {
    idx: i32,
    pq: i32,
    range: interval::interval_set::IntervalSet<u32>,
    raw_data: rust_htslib::bcf::Record,
}

struct VcfIndex {
    idx: i32,
    raw_data: rust_htslib::bcf::Record,
}

impl PartialEq for Vcf {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}

impl Eq for Vcf {}

impl PartialOrd for Vcf {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.pq.partial_cmp(&other.pq)
    }
}

impl Ord for Vcf {
    fn cmp(&self, other: &Self) -> Ordering {
        self.pq.cmp(&other.pq)
    }
}


impl PartialEq for VcfIndex {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}

impl Eq for VcfIndex {}

impl PartialOrd for VcfIndex {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.idx.partial_cmp(&other.idx)
    }
}

impl Ord for VcfIndex {
    fn cmp(&self, other: &Self) -> Ordering {
        self.idx.cmp(&other.idx)
    }
}


trait HasRawData {
    fn raw_data(&self) -> &rust_htslib::bcf::Record;
}

impl HasRawData for Vcf {
    fn raw_data(&self) -> &rust_htslib::bcf::Record {
        return &self.raw_data;
    }
}

impl HasRawData for VcfIndex {
    fn raw_data(&self) -> &rust_htslib::bcf::Record {
        return &self.raw_data;
    }
}

fn genotypes(record: &mut rust_htslib::bcf::Record) -> Option<rust_htslib::bcf::record::Genotypes> {
    return record.genotypes().ok();
}

fn print_usage(program: &str, opts: Options) {
    let brief = format!("Usage: {} FILE [options]", program);
    print!("{}", opts.usage(&brief));
}

#[derive(Clone)]
pub struct Args {
    pub flag_thread: bool,
    cmd_output: bool,
    arg_output: Option<String>,
    arg_input: String,
    cmd_chr: bool,
    arg_chr: Option<String>, /* flag_help: bool,
                              * flag_version: bool */
}

static VERSION: &'static str = concat!("##vcf-phenotype-quality-filter=", env!("CARGO_PKG_VERSION"));


/// Call to parse args.
pub fn option_parser() -> Option<Args> {
    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.optopt("o", "", "set output file name", "FILE");
    opts.optopt("c", "chr", "filiter by chromosome id", "CHRID");
    opts.optflag("t", "thread", "enable multi-thread execution");
    opts.optflag("h", "help", "print this help menu");
    opts.optflag("v", "version", "print version info");
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => panic!(f.to_string()),
    };
    if matches.opt_present("h") {
        print_usage(&program, opts);
        return None;
    }
    if matches.opt_present("v") {
        println!("{}", VERSION);
        return None;
    }
    let output = matches.opt_str("o");
    let thread = matches.opt_present("t");
    let chr = matches.opt_str("c");
    let input = if !matches.free.is_empty() {
        matches.free[0].clone()
    } else {
        print_usage(&program, opts);
        return None;
    };
    return Some(Args {
        cmd_chr: chr.is_some(),
        flag_thread: thread,
        cmd_output: output.is_some(),
        arg_input: input,
        arg_output: output,
        arg_chr: chr,
    });
}

/// Call to run with single thread.
pub fn run_sequencial(mut args: &mut Args) {
    let mut output_path = if args.cmd_output {
        args.arg_output.clone().unwrap()
    } else {
        args.arg_input.clone()
    };
    {
        let args_m: &mut Args = &mut args;
        solve_chromosome(0,
                         &mut args_m.arg_input,
                         &mut output_path,
                         &mut args_m.arg_chr);
    }
    solve_chromosome(1, &mut args.arg_input, &mut output_path, &mut args.arg_chr);
}

/// Call to run with 2 threads.
pub fn run<'a>() {
    debug!("Running Multi-Thread");
    // Input should be sorted by vcf-sort, and normalized by bcftools norm, using single sample.

    let handle1 = thread::spawn(move || {
        let mut args = option_parser().unwrap();
        let mut output_path = if args.cmd_output {
            args.arg_output.unwrap()
        } else {
            args.arg_input.clone()
        };
        solve_chromosome(0, &mut args.arg_input, &mut output_path, &mut args.arg_chr);
    });
    let handle2 = thread::spawn(move || {
        let mut args = option_parser().unwrap();
        let mut output_path = if args.cmd_output {
            args.arg_output.unwrap()
        } else {
            args.arg_input.clone()
        };
        solve_chromosome(1, &mut args.arg_input, &mut output_path, &mut args.arg_chr);
    });
    let result = handle1.join();
    let result2 = handle2.join();
    assert!(!result.is_err());
    assert!(!result2.is_err());
}

fn with_suffix(filepath: &str, suffix: &String, extension: &'static str) -> String {
    let mut pathbuf = String::new();
    pathbuf.push_str(filepath);

    pathbuf.push_str("_");
    pathbuf.push_str(suffix);
    pathbuf.push_str(extension);
    return pathbuf;
}

fn gen_output_filename<'a>(output_file: &'a String,
                           suffix: &'a String,
                           header: &'a bcf::Header)
                           -> Result<bcf::Writer, &'a String> {
    let regex_vcfgz = Regex::new("(.*).vcf.gz$").unwrap();
    let regex_vcf = Regex::new("(.*).vcf$").unwrap();
    let regex_bcf = Regex::new("(.*).bcf$").unwrap();
    let vcfgz = regex_vcfgz.captures(output_file);
    let vcf = regex_vcf.captures(output_file);
    let bcf = regex_bcf.captures(output_file);

    match vcfgz {
        Some(a) => {
            let k = with_suffix(a.get(1).unwrap().as_str(), suffix, ".vcf.gz");
            return Ok(bcf::Writer::from_path(&k, header, false, true)
                      .ok()
                      .expect("Error opening VCF"));
        }
        None => {
            match vcf {
                Some(a) => {
                    let k = with_suffix(a.get(1).unwrap().as_str(), suffix, ".vcf");
                    return Ok(bcf::Writer::from_path(&k, header, true, true)
                        .ok()
                        .expect("Error opening VCF"));
                }
                None => {
                    match bcf {
                        Some(a) => {
                            let k = with_suffix(a.get(1).unwrap().as_str(), suffix, ".bcf");
                            return Ok(bcf::Writer::from_path(&k, header, true, false)
                                .ok()
                                .expect("Error opening VCF"));
                        }
                        None => return Err(output_file),
                    }
                }
            }
        }
    }
}

fn solve_chromosome<'a>(haploid: usize,
                        input_file: &mut String,
                        output_file: &mut String,
                        chr: &mut Option<String>) {

    let input_file = Path::new(&input_file);
    if !input_file.exists() {
        println!("Error: No input file.");
        return;
    }
    let mut bcf: bcf::Reader = bcf::Reader::from_path(&input_file).ok().expect("Error opening vcf.");
    let mut header = bcf::Header::with_template(&bcf.header());
    let header2 = header.push_record(VERSION.as_bytes());

    // Last 2 Argument means (uncompressed: bool, vcf: bool)
    let mut out = gen_output_filename(&output_file, &haploid.to_string(), header2)
        .ok()
        .expect("Error opening vcf.");

    let mut heap = BinaryHeap::new();
    let mut index = 0;
    let mut previous_end = 0;
    let mut previous_rid = None;
    let chr_rid = match chr {
        &mut Some(ref a) => bcf.header().name2rid(a.as_bytes()).ok(),
        &mut None => None,
    };

    for r in bcf.records() {
        let mut record = r.ok().expect("Error reading Vcf file.");
        let rid = record.rid();
        if chr_rid.is_some() && chr_rid != rid {
            continue;
        }
        out.translate(&mut record);
        out.subset(&mut record);

        let pos = record.pos();
        let alt = record.inner().rlen;
        debug!("x={} {}", pos, alt);
        let end = pos + alt as u32;
        let interval = interval::IntervalSet::new(pos + 1, end);

        if previous_end < pos || previous_rid != rid {
            debug!("Resove duplications at {}", pos);
            resolve_duplication(&mut heap, &mut out);
            heap.clear();
            // if pos > 811368 { break; }
        }

        let pq = {
            let pq_opt = record.format(b"PQ").integer();
            match pq_opt {
                Ok(a) => a[0][0],
                Err(_) => 0,
            }
        };
        let genotype = genotypes(&mut record).unwrap().get(0);
        if genotype[haploid].index().unwrap() == 1 {
            heap.push(Vcf {
                idx: index,
                raw_data: record,
                pq: pq,
                range: interval,
            });
        }
        index += 1;
        previous_end = end;
        previous_rid = rid;
    }
    resolve_duplication(&mut heap, &mut out);
}

/*fn resolve_dup(mut heap: &mut BinaryHeap<Vcf>, out: &mut Writer) {
    if heap.len() > 1{
        heap = resolve_duplication(heap);
    }
    output_from_heap(heap, out)
}*/

fn resolve_duplication(mut heap: &mut BinaryHeap<Vcf>, out: &mut Writer) {
    if heap.len() == 1 {
        output_from_heap(&mut heap, out)
    } else {
        let mut output_heap = BinaryHeap::new();
        let mut intervalset = vec![].to_interval_set();

        while let Some(Vcf { pq, idx, raw_data, range }) = heap.pop() {
            if range.upper() - range.lower() == 0 && intervalset.overlap(&range.upper()) {
                info!("#Remove SNPs at {}, quality: {}", range, pq);
            } else if intervalset.overlap(&range) {
                info!("#Remove SNPs at {}, quality: {}", range, pq);
            } else {
                debug!("{}, {}", pq, range);
                intervalset = intervalset.union(&range);
                output_heap.push(VcfIndex {
                    idx: idx,
                    raw_data: raw_data,
                });
            }
        }
        output_from_heap(&mut output_heap, out)
    }
}

fn output_from_heap<T: Ord + HasRawData>(output_heap: &mut BinaryHeap<T>, out: &mut Writer) {
    while let Some(a) = output_heap.pop() {
        out.write(&a.raw_data()).ok().expect("Error Writing Vcf file.");
    }
}

#[test]
fn with_suffix_works() {
    assert_eq!(with_suffix("/root/foo",&1.to_string(),".txt"), "/root/foo_1.txt");
}

#[test]
fn gen_output_filename_works(){
    let h = bcf::Header::new();
    assert_eq!(gen_output_filename(&0.to_string(),&1.to_string(),&h), true)
}
