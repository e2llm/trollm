import json
from pathlib import Path
import unittest
from jsonschema import Draft202012Validator

SCHEMA = json.loads((Path(__file__).resolve().parents[1] / 'schemas/task-result-artifact.v1.schema.json').read_text())

class TaskArtifactSchemaTests(unittest.TestCase):
    def test_legacy_tasks_and_explicit_single_read_shape(self):
        Draft202012Validator.check_schema(SCHEMA)
        validator = Draft202012Validator(SCHEMA)
        validator.validate({'model': 'reasoner', 'tools': []})
        validator.validate({'resultArtifact': 'selected-original', 'tools': ['service.read'], 'completion': 'tool-result'})

    def test_invalid_names_modes_and_tool_counts(self):
        validator = Draft202012Validator(SCHEMA)
        good = {'resultArtifact': 'selected-original', 'tools': ['service.read'], 'completion': 'tool-result'}
        invalid = [{'resultArtifact': x} for x in [None, '', '../escape', 'calculation', 'model-failure-any', 'a' * 65]]
        invalid += [{'tools': x} for x in [[], ['local.calculate'], ['flow.choose'], ['service.read', 'service.other']]]
        invalid += [{'completion': 'tool-or-route'}, {'loop': {'maxSteps': 2}}]
        for change in invalid:
            with self.subTest(change=change):
                self.assertTrue(list(validator.iter_errors(good | change)))

if __name__ == '__main__':
    unittest.main()
