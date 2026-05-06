# Extraction Evaluation

Measures extraction quality using Precision, Recall, F1.
Also tracks sensitivity classification accuracy and entity extraction quality.

## Dataset

200 synthetic conversations with 240 expected memory proposals.
Includes 40 PII and 40 secret-containing conversations.

```bash
python3 eval/extraction/generate_dataset.py   # regenerate
```

## Run

```bash
# Mock baseline (perfect extraction with noise)
python3 eval/extraction/run.py --mock --save-baseline

# Against live server (not yet implemented)
python3 eval/extraction/run.py --endpoint http://127.0.0.1:7777
```

## Baseline

Mock extraction baseline: F1 = 1.0000, Sensitivity accuracy = 0.8333.
Target from DoD: F1 >= 0.7.
