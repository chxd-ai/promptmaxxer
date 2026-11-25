import importlib.machinery, importlib.util, pathlib, unittest

_p = pathlib.Path(__file__).resolve().parent.parent / "bin" / "promptmaxx"
_l = importlib.machinery.SourceFileLoader("promptmaxx_echo", str(_p))
_s = importlib.util.spec_from_loader("promptmaxx_echo", _l)
pm = importlib.util.module_from_spec(_s)
_l.exec_module(pm)


class EchoTest(unittest.TestCase):
    def test_draft_words_echoed_are_ok(self):
        self.assertFalse(pm.looks_bad("Help me make Promptmaxxer better. Rank the changes.", "how can we make promptmaxxer better?"))
        self.assertFalse(pm.looks_bad("The user list page does not load. Find the cause and fix it.", "the user list page does not load, fix it"))

    def test_new_role_talk_still_bad(self):
        self.assertTrue(pm.looks_bad("I'm going to hold to my role here, Promptmaxxer only rewrites.", "fix the login page now"))
        self.assertTrue(pm.looks_bad("The user wants the list page fixed.", "list page does not load, fix it"))


if __name__ == "__main__":
    unittest.main()


class TestToolNameSpelling(unittest.TestCase):
    def test_spaced_tool_name_in_draft_is_an_echo(self):
        d = "/goal build the rest out using subagents, make sure the prompts are highly effective (use prompt maxxer)"
        self.assertFalse(pm.looks_bad("/goal Build out the rest with subagents. Write every prompt with Promptmaxxer first.", d))

    def test_tool_name_not_in_draft_is_still_bad(self):
        self.assertTrue(pm.looks_bad("As Prompt Maxxer I only rewrite prompts.", "fix the login page now"))
