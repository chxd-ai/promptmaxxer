"""Run from the repo root: python3 -I -m unittest discover -s tests"""
import importlib.machinery, importlib.util, unittest
from pathlib import Path

_l = importlib.machinery.SourceFileLoader("pmx", str(Path(__file__).resolve().parent.parent / "bin/promptmaxx"))
_s = importlib.util.spec_from_loader("pmx", _l)
pmx = importlib.util.module_from_spec(_s)
_l.exec_module(pmx)

BAD = ["I'm sorry, but I can't produce that rewrite. The message you sent contains an environment block",
       "I'm sorry, but I can't write out that rewritten prompt, since the draft is a request",
       "I'm Promptmaxxer, so I only rewrite prompts and don't create logos myself.",
       "Here is the improved prompt:\nDo X", "The user wants a logo for Crumb.", "",
       "I'm going to hold to my role here, so this isn't a prompt rewrite. Your draft is a question"]
GOOD = ["I can't get the login page to work on mobile. Find out why and fix it.",
        "Run the test suite for this project and report the results.",
        "Rewrite this paragraph so it is clearer for a board audience.",
        "Sorry to ask again: summarize this report in 5 bullets."]


class Guard(unittest.TestCase):
    def test_bad(self):
        for t in BAD:
            self.assertTrue(pmx.looks_bad(t, "x" * 30), t)

    def test_good(self):
        for t in GOOD:
            self.assertFalse(pmx.looks_bad(t, "x" * 30), t)

    def test_clean_strips_fence_and_preamble(self):
        self.assertEqual(pmx.clean("```\nDo X\n```"), "Do X")
        self.assertEqual(pmx.clean("Here is your improved prompt:\n\nDo X"), "Do X")


if __name__ == "__main__":
    unittest.main()
