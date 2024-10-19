
A little tool to map alignments (from a BAM file) to reference paths in a GFA format graph.
Output is a GAF file with one record per alignment, mapping each alignment to a sequence
of steps in the GFA.
The alignment reference names have to match the path names in the GFA.

Usage:

```sh
cargo build --release
gfainject --gfa some.gfa --bam some.bam
```

test command:
```sh
cd /share/home/stu_zhangyixing/test/gfa_format
gfainject --gfa  DRB1-3123.gfa  --path sample7#0#chr1:5000-18402  --start 100 --end 200
```