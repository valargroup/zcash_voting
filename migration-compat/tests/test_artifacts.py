"""The oracle must reject convincing but damaged captures."""
import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('migration_run', Path(__file__).parents[1] / 'run.py')
suite = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite)


class ArtifactTests(unittest.TestCase):
    def test_same_length_blob_substitution_is_detected(self):
        before = {'bundles': (['round_id', 'van_comm_rand'], [('round', b'abc')])}
        after = {'bundles': (['round_id', 'van_comm_rand'], [('round', b'abd')])}
        with self.assertRaisesRegex(AssertionError, 'van_comm_rand'):
            suite.assert_preserved(before, after)

    def test_row_loss_is_detected(self):
        with self.assertRaisesRegex(AssertionError, 'row count'):
            suite.assert_preserved({'votes': (['choice'], [(0,), (1,)])}, {'votes': (['choice'], [(0,)])})

    def test_changed_choice_is_detected(self):
        with self.assertRaisesRegex(AssertionError, 'choice'):
            suite.assert_preserved({'votes': (['choice'], [(0,)])}, {'votes': (['choice'], [(1,)])})

    def test_only_recovery_metadata_normalization_is_allowed(self):
        before = {'votes': (['commitment_bundle_json'], [('{"format":"zcash_voting_vote_recovery_v1","vote_decision":0}',)])}
        good = '{"format":"zcash_voting_vote_recovery_v1","vote_decision":0,"batch_digest":null,"batch_index":null,"batch_size":null}'
        suite.assert_preserved(before, {'votes': (['commitment_bundle_json'], [(good,)])})
        with self.assertRaisesRegex(AssertionError, 'recovery semantics'):
            suite.assert_preserved(before, {'votes': (['commitment_bundle_json'], [(good.replace('decision":0', 'decision":1'),)])})

    def test_backup_keeps_committed_wal_rows(self):
        with tempfile.TemporaryDirectory() as temp:
            source, copy = Path(temp) / 'source.sqlite', Path(temp) / 'copy.sqlite'
            with sqlite3.connect(source) as db:
                db.execute('PRAGMA journal_mode=WAL')
                db.execute('PRAGMA wal_autocheckpoint=0')
                db.execute('CREATE TABLE votes(choice INTEGER)')
                db.execute('INSERT INTO votes VALUES(1)')
                db.commit()
                self.assertTrue(Path(str(source) + '-wal').exists())
                suite.copy_database(source, copy)
                self.assertEqual(suite.snapshot(source), suite.snapshot(copy))

    def test_missing_source_is_not_created(self):
        with tempfile.TemporaryDirectory() as temp:
            missing = Path(temp) / 'missing.sqlite'
            with self.assertRaises(sqlite3.OperationalError):
                suite.connect(missing)
            self.assertFalse(missing.exists())

    def test_schema_normalization_keeps_constraint_string_spaces(self):
        with tempfile.TemporaryDirectory() as temp:
            paths = [Path(temp) / name for name in ['a.sqlite', 'b.sqlite']]
            for path, value in zip(paths, ['a b', 'ab']):
                with sqlite3.connect(path) as db:
                    db.execute(f"CREATE TABLE t(value TEXT CHECK(value='{value}'))")
            self.assertNotEqual(suite.schema(paths[0]), suite.schema(paths[1]))
