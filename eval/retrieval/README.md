# Retrieval Evaluation

Measures retrieval quality using NDCG@k, MRR, and Recall@k.

## Dataset

200 queries across 5 types: factual, entity, topic, temporal, negation.
200 synthetic memory documents as corpus.

```bash
python3 eval/retrieval/generate_dataset.py   # regenerate
```

## Run

```bash
# Mock baseline (keyword matching)
python3 eval/retrieval/run.py --mock --save-baseline

# Against live server
python3 eval/retrieval/run.py --endpoint http://127.0.0.1:7777

# Regression check (fails if NDCG@10 drops >2%)
python3 eval/retrieval/run.py --endpoint http://... --check-regression
```

## Baseline

Mock retrieval baseline: NDCG@10 = 0.8000, MRR = 0.8000, Recall@10 = 1.0000.
