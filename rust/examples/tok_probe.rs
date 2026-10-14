//! 一次性探针：与 python tokenizers（模块 0.22.2）对拍分词结果。
//! cargo run --release --example tok_probe --features embed-server -- "文本1" "文本2"

use tokenizers::tokenizer::{
    PaddingParams, PaddingStrategy, Tokenizer, TruncationDirection, TruncationParams,
    TruncationStrategy,
};
use tokenizers::PaddingDirection;

fn main() {
    let mut tok = Tokenizer::from_file("/home/fslong/.rsrs/models/bge-m3/tokenizer.json")
        .expect("load tokenizer");
    tok.with_truncation(Some(TruncationParams {
        max_length: 512,
        strategy: TruncationStrategy::LongestFirst,
        stride: 0,
        direction: TruncationDirection::Right,
    }))
    .expect("truncation");
    tok.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        direction: PaddingDirection::Right,
        pad_to_multiple_of: None,
        pad_id: 0,
        pad_type_id: 0,
        pad_token: "[PAD]".to_string(),
    }));
    for t in std::env::args().skip(1) {
        let e = tok
            .encode(tokenizers::EncodeInput::Single(t.clone().into()), true)
            .expect("encode");
        let ids = e.get_ids();
        let head = &ids[..40.min(ids.len())];
        println!("{} {:?}", ids.len(), head);
    }
}
