You can ask a decision model with the `decide` bash function. A decision model (Jev, Clef, or one OpenRouter routes to: System One models) reads a state and typed questions and answers each with calibrated probabilities in one pass. It generates no text, so it is for judgments a turn needs before acting: triage, routing, yes/no screens, severity ratings. Three question kinds: `noul` (the probability a yes/no statement is true), `choice` (one of the options you define, with a probability per option), `score` (a position on an ordered rubric you write).

    decide noul "Customer: my order never arrived, I want a refund now" "Does the customer ask for a person to step in?"
    decide choice "Customer: my order never arrived, I want a refund now" "Which team handles this" billing="Payments and refunds" logistics="Shipping and delivery"
    decide score "Checkout has failed for every customer for an hour." "How severe is the impact" "No impact" "Minor" "Major" "Critical"

One line per answer: the pick and its probability, the whole distribution in brackets, the confidence where the model reports one. A noul reads `yes` above 0.9 and `no` below 0.1, `uncertain` between; the number is the answer, the word is a reading of it.

Several questions over one state, or a structured state, go in one JSON request on stdin (ask every question you might need in one call; they are scored together):

    decide <<'JSON'
    {
      "state": {"ticket": "refunded twice, need help", "tier": "pro"},
      "questions": {
        "urgent": {"type": "noul", "instructions": "Does this need an answer today?"},
        "team": {"type": "choice", "instructions": "Which team handles it",
                 "criteria": {"billing": "Payments and refunds", "technical": "Bugs and outages"}},
        "severity": {"type": "score", "instructions": "Customer impact",
                     "criteria": ["None", "One order", "Many orders", "All orders"]}
      }
    }
    JSON

Flags come before the verb: `-f FILE` reads the request from a file instead of stdin, `--json` prints the raw response instead of the lines (`decide --json noul ...`). The provider and model are the profile's, its default decision instance (`decision.instance` in the profile); you do not pick them, the profile does. Exit codes: 0 answered, 1 the provider failed, 2 usage, 3 the request was refused (fix it, a retry with the same request fails the same way), 4 not wired in this shell.