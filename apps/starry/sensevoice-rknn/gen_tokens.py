#!/usr/bin/env python3
"""Generate sherpa-onnx-format tokens.txt from the SenseVoice bpe.model.

The runtime rootfs only carries numpy (no sentencepiece), so we build the
id->surface table at prebuild time and ship tokens.txt alongside the model.
"""
import sys
import sentencepiece as spm


def main():
    model_cache = sys.argv[1]
    bpe_path = model_cache + "/chn_jpn_yue_eng_ko_spectok.bpe.model"
    sp = spm.SentencePieceProcessor()
    sp.load(bpe_path)
    vocab = sp.get_piece_size()
    with open(model_cache + "/tokens.txt", "w", encoding="utf-8") as f:
        for i in range(vocab):
            f.write(sp.id_to_piece(i) + " " + str(i) + "\n")
    print("tokens.txt generated, vocab=" + str(vocab))


if __name__ == "__main__":
    main()
