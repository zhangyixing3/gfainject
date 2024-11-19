use anyhow::Result;
use roaring::RoaringBitmap;
use std::collections::BTreeMap;
use std::io::prelude::*;
use std::{io::BufReader, path::PathBuf};

#[derive(Debug)]
struct Args {
    gfa: PathBuf,
    alignments: Option<PathBuf>,

    path_range: Option<(String, usize, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PathStep {
    node: u32,
    reverse: bool,
}

struct PathIndex {
    segment_id_range: (usize, usize),
    segment_lens: Vec<usize>,

    path_names: BTreeMap<String, usize>,
    // path_names: Vec<String>,
    path_steps: Vec<Vec<PathStep>>,

    // path_step_offsets: Vec<Vec<usize>>,
    path_step_offsets: Vec<roaring::RoaringBitmap>,
}

struct PathStepRangeIter<'a> {
    path_id: usize,
    pos_range: std::ops::Range<u32>,
    // start_pos: usize,
    // end_pos: usize,
    steps: Box<dyn Iterator<Item = (usize, &'a PathStep)> + 'a>,
    // first_step_start_pos: u32,
    // last_step_end_pos: u32,
}

impl<'a> Iterator for PathStepRangeIter<'a> {
    type Item = (usize, &'a PathStep);

    fn next(&mut self) -> Option<Self::Item> {
        self.steps.next()
    }
}

impl PathIndex {
    fn path_step_range_iter<'a>(
        &'a self,
        path_name: &str,
        pos_range: std::ops::Range<u32>,
    ) -> Option<PathStepRangeIter<'a>> {
        let path_id = *self.path_names.get(path_name)?;
        let offsets = self.path_step_offsets.get(path_id)?;

        let start = pos_range.start;
        let end = pos_range.end;
        let start_rank = offsets.rank(start);
        let end_rank = offsets.rank(end);

        let steps = {
            let path_steps = self.path_steps.get(path_id)?;

            let skip = (start_rank as usize).checked_sub(1).unwrap_or_default();
            let take = end_rank as usize - skip;
            let iter = path_steps
                .iter()
                .skip(skip)
                .take(take)
                .enumerate()
                .map(move |(ix, step)| (skip + ix, step))
                .fuse();

            Box::new(iter) as Box<dyn Iterator<Item = _>>
        };

        Some(PathStepRangeIter {
            path_id,
            pos_range,
            steps,
            // first_step_start_pos,
            // last_step_end_pos,
        })
    }

    fn from_gfa(gfa_path: impl AsRef<std::path::Path>) -> Result<Self> {
        let gfa = std::fs::File::open(&gfa_path)?;
        let mut gfa_reader = BufReader::new(gfa);

        let mut line_buf = Vec::new();

        let mut seg_lens = Vec::new();

        let mut seg_id_range = (std::usize::MAX, 0usize);
        // dbg!();

        loop {
            line_buf.clear();

            let len = gfa_reader.read_until(0xA, &mut line_buf)?;
            if len == 0 {
                break;
            }

            let line = &line_buf[..len];
            let line_str = std::str::from_utf8(&line)?;
            // println!("{line_str}");

            if !matches!(line.first(), Some(b'S')) {
                continue;
            }

            let mut fields = line_str.split(|c| c == '\t');

            let Some((name, seq)) = fields.next().and_then(|_type| {
                let name = fields.next()?.trim();
                let seq = fields.next()?.trim();
                Some((name, seq))
            }) else {
                continue;
            };
            let seg_id = name.parse::<usize>()?;

            seg_id_range.0 = seg_id_range.0.min(seg_id);
            seg_id_range.1 = seg_id_range.1.max(seg_id);

            let len = seq.len();
            seg_lens.push(len);
        }

        assert!(
        seg_id_range.1 - seg_id_range.0 == seg_lens.len() - 1,
        "GFA segments must be tightly packed: min ID {}, max ID {}, node count {}, was {}",
        seg_id_range.0, seg_id_range.1, seg_lens.len(),
        seg_id_range.1 - seg_id_range.0,
        );

        let gfa = std::fs::File::open(&gfa_path)?;
        let mut gfa_reader = BufReader::new(gfa);

        let mut path_names = BTreeMap::default();

        let mut path_steps: Vec<Vec<PathStep>> = Vec::new();
        let mut path_step_offsets: Vec<RoaringBitmap> = Vec::new();
        // let mut path_pos: Vec<Vec<usize>> = Vec::new();

        loop {
            line_buf.clear();

            let len = gfa_reader.read_until(b'\n', &mut line_buf)?;
            if len == 0 {
                break;
            }

            let line = &line_buf[..len];
            if !matches!(line.first(), Some(b'P')) {
                continue;
            }

            let mut fields = line.split(|&c| c == b'\t');

            let Some((name, steps)) = fields.next().and_then(|_type| {
                let name = fields.next()?;
                let steps = fields.next()?;
                Some((name, steps))
            }) else {
                continue;
            };

            let name = std::str::from_utf8(name)?;
            path_names.insert(name.to_string(), path_steps.len());

            let mut pos = 0;

            let mut parsed_steps = Vec::new();

            let mut offsets = RoaringBitmap::new();

            let steps = steps.split(|&c| c == b',');

            for step in steps {
                let (seg, orient) = step.split_at(step.len() - 1);
                let seg_id = btoi::btou::<usize>(seg)?;
                let seg_ix = seg_id - seg_id_range.0;
                let len = seg_lens[seg_ix];

                let is_rev = orient == b"+";

                let step = PathStep {
                    node: seg_ix as u32,
                    reverse: is_rev,
                };
                parsed_steps.push(step);
                offsets.push(pos as u32);

                pos += len;
            }

            path_steps.push(parsed_steps);
            path_step_offsets.push(offsets);
        }

        Ok(Self {
            // 简单来说这段程序为GFA文件建立索引，从而快速索引坐标
            // 读取GFA文件P开头的行，记录path_name和node 的偏移量
            // let mut path_names = BTreeMap::default();
            // path_names.insert(name.to_string(), path_steps.len()); (path_1：1 ,path_2:2，path_3:3)
            path_names,
            // let step = PathStep {
            //     node: seg_ix as u32,
            //     reverse: is_rev,
            // };
            // parsed_steps.push(step);
            // path_steps.push(parsed_steps);
            path_steps,
            //  let mut offsets = RoaringBitmap::new();
            //  offsets.push(pos as u32); Node positios of line_refgenome
            //  let mut path_step_offsets: Vec<RoaringBitmap> = Vec::new();
            //  path_step_offsets.push(offsets);
            path_step_offsets,

            segment_id_range: seg_id_range, // Node ID range of the segments in the GFA file (1, MAX)
            segment_lens: seg_lens, // Node lengths of the segments in the GFA file [1,4,3,32,32,32]
        })
    }
}

fn main() -> Result<()> {
    let Ok(args) = parse_args() else {
        println!("USAGE: `gfainject --gfa <gfa-path> --bam <bam-path>`");
        return Ok(());
    };

    let path_index = PathIndex::from_gfa(&args.gfa)?;

    if let Some(bam_path) = args.alignments {
        return main_cmd(path_index, bam_path);
    } else if let Some((path, start, end)) = args.path_range {
        return path_range_cmd(path_index, path, start, end);
    }

    Ok(())
}

fn path_range_cmd(
    path_index: PathIndex,
    path_name: String,
    start: usize,
    end: usize,
) -> Result<()> {
    // output:
        // start_rank: 13
        // end_rank: 27
        // cardinality: 14
        // ------
        // step_ix	node	pos
        // 12	4935	1	4936	63
        // 13	4934	1	4935	108
        // 14	4932	1	4933	109
        // 15	4930	1	4931	110
        // 16	4929	1	4930	111
        // 17	4928	1	4929	131
        // 18	4926	1	4927	132
        // 19	4925	1	4926	135
        // 20	4923	1	4924	136
        // 21	4921	1	4922	137
        // 22	4920	1	4921	179
        // 23	4918	1	4919	180
        // 24	4915	1	4916	191
        // 25	4914	1	4915	192
        // 26	4913	1	4914	193
        // ------------
        // 12	4936	63
        // 13	4935	108
        // 14	4933	109
        // 15	4931	110
        // 16	4930	111
        // 17	4929	131
        // 18	4927	132
        // 19	4926	135
        // 20	4924	136
        // 21	4922	137
        // 22	4921	179
        // 23	4919	180
        // 24	4916	191
        // 25	4915	192
        // 26	4914	193
    let path = path_index
        .path_names
        .get(&path_name)
        .expect("Path not found");

    let offsets = path_index.path_step_offsets.get(*path).unwrap();
    let steps = path_index.path_steps.get(*path).unwrap();

    let start_rank = offsets.rank(start as u32);
    let end_rank = offsets.rank(end as u32);

    // 例子：假设数据结构和输入
    // 假设：
    // offsets = [0, 10, 20, 30, 40, 50]，表示每个步骤的偏移位置。
    // steps = [{node: 1}, {node: 2}, {node: 3}, {node: 4}, {node: 5}, {node: 6}]，表示路径步骤。
    // start = 15，end = 45

    // 运行流程：
    // rank(15) -> 2（表示位置 15 前有两个有效项）。
    // rank(45) -> 5（表示位置 45 前有五个有效项）。
    // skip = 2 - 1 = 1，take = 5 - 1 = 4。
    // 遍历 offsets 和 steps 的第 2 项到第 5 项

    let cardinality = offsets.range_cardinality((start as u32)..(end as u32));

    println!("start_rank: {start_rank}");
    println!("end_rank: {end_rank}");
    println!("cardinality: {cardinality}");

    println!("------");
    let skip = (start_rank as usize).checked_sub(1).unwrap_or_default();
    let take = end_rank as usize - skip;


    println!("step_ix\tnode\tpos");
    // let skip = 0;
    // let take = 20;
    for (step_ix, (pos, step)) in
        offsets.iter().zip(steps).enumerate().skip(skip).take(take)
    {
        let node = step.node + path_index.segment_id_range.0 as u32;
		let aa = step.node;
		let bb = path_index.segment_id_range.0 as u32;
        println!("{step_ix}\t{aa}\t{bb}\t{node}\t{pos}");
    }

    println!("------------");

    // let start_pos = start.get() as u32;
    // let start_rank = path_index.path_step_offsets[path_id].rank(start_pos);
    // let step_offset = start_pos
    //     - path_index.path_step_offsets[path_id]
    //         .select((start_rank - 1) as u32)
    //         .unwrap();

    // let pos_range = (start.get() as u32)..(1 + end.get() as u32);
    let pos_range = (start as u32)..(end as u32);
    if let Some(steps) = path_index.path_step_range_iter(&path_name, pos_range)
    {
        for (step_ix, step) in steps {
            let pos = offsets.select(step_ix as u32).unwrap();
            //
            let node = step.node + path_index.segment_id_range.0 as u32;
            println!("{step_ix}\t{node}\t{pos}");
        }
    }

    Ok(())
}

fn main_cmd(path_index: PathIndex, bam_path: PathBuf) -> Result<()> {
    use noodles::bam;

    let Ok(args) = parse_args() else {
        println!("USAGE: `gfa-injection --gfa <gfa-path> --bam <bam-path>`");
        return Ok(());
    };

    let mut bam = std::fs::File::open(&bam_path).map(bam::Reader::new)?;

    let header = {
        use noodles::sam;
        // the noodles parse() impl demands that the @HD lines go first,
        // but that's clearly not a guarantee i can enforce
        let raw = bam.read_header()?;
        let mut header = sam::Header::builder();

        for line in raw.lines() {
            use noodles::sam::header::Record as HRecord;
            if let Ok(record) = line.parse::<HRecord>() {
                header = match record {
                    HRecord::Header(hd) => header.set_header(hd),
                    HRecord::ReferenceSequence(sq) => {
                        header.add_reference_sequence(sq)
                    }
                    HRecord::ReadGroup(rg) => header.add_read_group(rg),
                    HRecord::Program(pg) => header.add_program(pg),
                    HRecord::Comment(co) => header.add_comment(co),
                };
            }
        }

        header.build()
    };

    let ref_seqs = bam.read_reference_sequences()?;

    // for (key, val) in ref_seqs {
    //     let len = val.length();
    //     eprintln!("{key}\t{len}");
    // }


    let mut stdout = std::io::stdout().lock();

    for rec in bam.records() {
        let record = rec?;

        // if record.flags().is_reverse_complemented() {
        //     continue;
        // }

        let Some(read_name) = record.read_name() else {
            continue;
        };

        // let name = read_name.to_string();
        // dbg!(&name);
        // let name = std::str::from_utf8(read_name.to_)?;

        // convenient for debugging
        // let name: &str = read_name.as_ref();
        // if name != "A00404:156:HV37TDSXX:4:1213:6370:23359" {
        //     continue;
        // }

        let Some(ref_name) = record
                    .reference_sequence(&header)
                    .and_then(|s| s.ok().map(|s| s.name()))
        else {
            continue;
        };

        let Some(path_id) = path_index.path_names.get(ref_name.as_str()).copied() else {
            continue;
        };

        let start = record.alignment_start().unwrap();
        let end = record.alignment_end().unwrap();
        let al_len = record.alignment_span();
        //assert!(end - start == al_len);

        let start_pos = (start.get()-1) as u32;
        let start_rank = path_index.path_step_offsets[path_id].rank(start_pos);
        //eprintln!("start_rank = {}", start_rank);
        let mut step_offset = start_pos
            - path_index.path_step_offsets[path_id]
                .select((start_rank - 1) as u32)
                .unwrap();

        let pos_range = ((start.get()-1) as u32)..((end.get()-1) as u32);
        if let Some(steps) =
            path_index.path_step_range_iter(ref_name.as_str(), pos_range)
        {
            let mut path_str = String::new();

            let mut steps = steps.collect::<Vec<_>>();

            if record.flags().is_reverse_complemented() {
                steps.reverse();
            }

            let mut path_len: usize = 0;

            for (_step_ix, step) in steps {
                // path length is given by the length of nodes in the graph
                path_len += path_index.segment_lens[(step.node) as usize];
                use std::fmt::Write;
                // eprintln!("step_ix: {step_ix}");

                // let reverse = step.reverse;
                let forward = step.reverse ^ record.flags().is_reverse_complemented();
                if forward {
                    write!(&mut path_str, ">")?;
                } else {
                    write!(&mut path_str, "<")?;
                }
                write!(
                    &mut path_str,
                    "{}",
                    step.node + path_index.segment_id_range.0 as u32
                )?;
            }

            if record.flags().is_reverse_complemented() {
                // start node offset changes
                //println!("is rev {} {} {}", path_len, step_offset, record.cigar().alignment_span());
                let last_bit = path_len as u32 - (step_offset as u32 + record.cigar().alignment_span() as u32 - 1);
                step_offset = last_bit;
            }

            // query name
            write!(stdout, "{}\t", read_name)?;

            // query len
            let query_len = record.cigar().read_length();
            write!(stdout, "{query_len}\t")?;

            // query start (0-based, closed)
            let query_start = 0;
            write!(stdout, "{query_start}\t")?;

            // query end (0-based, open)
            write!(stdout, "{}\t", query_start + query_len)?;

            // strand
            // if record.flags().is_reverse_complemented() {
            // print!("-\t");
            // } else {
            write!(stdout, "+\t")?;
            // }

            // path
            write!(stdout, "{path_str}\t")?;
            // path length
            write!(stdout, "{path_len}\t")?;
            // start on path
            let path_start = step_offset as usize;
            write!(stdout, "{path_start}\t")?;
            // end on path
            let path_end = path_start + al_len;
            write!(stdout, "{path_end}\t")?;
            // number of matches
            {
                use noodles::sam::record::cigar::{op::Kind, Op};

                fn match_len(op: &Op) -> usize {
                    match op.kind() {
                        Kind::Match
                        | Kind::SequenceMatch
                        | Kind::SequenceMismatch => op.len(),
                        _ => 0,
                    }
                }
                let matches =
                    record.cigar().iter().map(match_len).sum::<usize>();
                write!(stdout, "{matches}\t")?;
            }
          // alignment block length
            write!(stdout, "{al_len}\t")?;
            // mapping quality
            {
                let score =
                    record.mapping_quality().map(|q| q.get()).unwrap_or(255u8);
                write!(stdout, "{score}\t")?;
            }

            // cigar
            write!(stdout, "cg:Z:{}", record.cigar())?;

            writeln!(stdout)?;
        } else {
        }
    }

    std::io::stdout().flush()?;

    Ok(())
}

fn parse_args() -> std::result::Result<Args, pico_args::Error> {
    let mut pargs = pico_args::Arguments::from_env();

    let path_range = pargs.opt_value_from_str("--path").and_then(
        |path: Option<String>| {
            let start: Option<usize> = pargs.opt_value_from_str("--start")?;
            let end: Option<usize> = pargs.opt_value_from_str("--end")?;
            let path_range = path.and_then(|path| {
                let path: String = path.into();
                let start = start?;
                let end = end?;
                Some((path, start, end))
            });
            Ok(path_range)
        },
    )?;

    let args = Args {
        gfa: pargs.value_from_os_str("--gfa", parse_path)?,
        alignments: pargs.opt_value_from_os_str("--bam", parse_path)?,
        path_range,
    };

    Ok(args)
}

fn parse_path(s: &std::ffi::OsStr) -> Result<std::path::PathBuf, &'static str> {
    Ok(s.into())
}
