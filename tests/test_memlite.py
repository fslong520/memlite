import json
import os
import shutil
import subprocess
import tempfile
import unittest

from memlite.store import Store
from memlite.search import search, tokenize


class StoreTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="memlite-test-")

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def test_put_get_delete(self):
        s = Store(self.dir)
        rid = s.put("测试记忆：Python 3.12 发布", title="py312", tags=["python"], project="demo")
        self.assertEqual(s.count(), 1)
        rec = s.get(rid)
        self.assertIn("Python 3.12", rec["content"])
        self.assertEqual(rec["tags"], ["python"])
        s.delete(rid)
        self.assertEqual(s.count(), 0)
        self.assertIn(rid, s.deleted)

    def test_reload_persistence(self):
        s = Store(self.dir)
        rid = s.put("持久化测试", title="t1")
        s2 = Store(self.dir)
        self.assertEqual(s2.count(), 1)
        self.assertEqual(s2.get(rid)["title"], "t1")

    def test_bad_line_tolerated(self):
        s = Store(self.dir)
        s.put("正常条")
        with open(os.path.join(self.dir, "memory.jsonl"), "a", encoding="utf-8") as f:
            f.write("{bad json\n")
            f.write('{"op":"put","id":"x1","ts":1,"title":"","content":"坏行后的正常条","tags":[],"project":""}\n')
        s2 = Store(self.dir)
        self.assertEqual(s2.bad_lines, 1)
        self.assertEqual(s2.count(), 2)

    def test_delete_tombstone_survives_reload(self):
        s = Store(self.dir)
        rid = s.put("将删")
        s.delete(rid)
        s2 = Store(self.dir)
        self.assertEqual(s2.count(), 0)
        self.assertNotIn(rid, s2.records)

    def test_empty_content_rejected(self):
        s = Store(self.dir)
        with self.assertRaises(ValueError):
            s.put("   ")

    def test_compact(self):
        s = Store(self.dir)
        rid1 = s.put("a")
        rid2 = s.put("b")
        s.delete(rid1)
        before = s.line_count()
        s.compact()
        s2 = Store(self.dir)
        self.assertEqual(s2.count(), 1)
        self.assertIsNotNone(s2.get(rid2))
        self.assertLess(s2.line_count(), before)


class SearchTest(unittest.TestCase):
    def test_tokenize_mixed(self):
        toks = tokenize("memlite 是 Python 写的")
        self.assertIn("memlite", toks)
        self.assertIn("python", toks)
        self.assertIn("写", toks)  # 单字整存

    def test_tokenize_cjk_bigram(self):
        toks = tokenize("记忆系统")
        self.assertIn("记忆", toks)
        self.assertIn("忆系", toks)
        self.assertIn("系统", toks)

    def test_search_ranking(self):
        recs = [
            {"id": "1", "ts": 1, "title": "rust 编译", "content": "cargo build 慢用 sccache", "tags": [], "project": ""},
            {"id": "2", "ts": 2, "title": "菜谱", "content": "番茄炒蛋的做法", "tags": [], "project": ""},
            {"id": "3", "ts": 3, "title": "", "content": "rust 编译缓存 sccache 配置", "tags": [], "project": ""},
        ]
        hits = search(recs, "rust 编译")
        self.assertTrue(hits)
        self.assertIn(hits[0][1]["id"], ("1", "3"))

    def test_search_title_weight(self):
        recs = [
            {"id": "1", "ts": 1, "title": "kubernetes 部署", "content": "无关内容", "tags": [], "project": ""},
            {"id": "2", "ts": 2, "title": "别的", "content": "提到一次 kubernetes", "tags": [], "project": ""},
        ]
        hits = search(recs, "kubernetes")
        self.assertEqual(hits[0][1]["id"], "1")

    def test_search_project_filter(self):
        recs = [
            {"id": "1", "ts": 1, "title": "", "content": "金秤 策略", "tags": [], "project": "goldscale"},
            {"id": "2", "ts": 2, "title": "", "content": "金秤 记忆", "tags": [], "project": "other"},
        ]
        hits = search(recs, "金秤", project="goldscale")
        self.assertEqual(len(hits), 1)
        self.assertEqual(hits[0][1]["id"], "1")

    def test_search_empty_query(self):
        self.assertEqual(search([{"id": "1", "content": "x", "tags": [], "ts": 1}], ""), [])


class SyncTest(unittest.TestCase):
    """用本地裸仓模拟 GitHub。"""

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="memlite-sync-")
        self.remote = os.path.join(self.dir, "remote.git")
        self.data = os.path.join(self.dir, "data")
        subprocess.run(["git", "init", "--bare", "-b", "main", self.remote], check=True, capture_output=True)
        os.makedirs(self.data)
        subprocess.run(["git", "init", "-b", "main", self.data], check=True, capture_output=True)
        subprocess.run(["git", "-C", self.data, "config", "user.email", "t@t"], check=True)
        subprocess.run(["git", "-C", self.data, "config", "user.name", "t"], check=True)
        subprocess.run(["git", "-C", self.data, "remote", "add", "origin", self.remote], check=True)

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def test_full_sync_roundtrip(self):
        from memlite.sync import sync

        s = Store(self.data)
        s.put("同步测试记忆")
        out = sync(self.data)
        self.assertTrue(out["verified"])
        self.assertEqual(out["local_sha"], out["remote_sha"])
        # 远端应能 clone 出同一条
        clone = os.path.join(self.dir, "clone")
        subprocess.run(["git", "clone", self.remote, clone], check=True, capture_output=True)
        s2 = Store(clone)
        self.assertEqual(s2.count(), 1)

    def test_two_way_sync(self):
        from memlite.sync import sync

        s = Store(self.data)
        s.put("机一的条")
        sync(self.data)
        # 模拟机二
        m2 = os.path.join(self.dir, "m2")
        subprocess.run(["git", "clone", self.remote, m2], check=True, capture_output=True)
        subprocess.run(["git", "-C", m2, "config", "user.email", "t@t"], check=True)
        subprocess.run(["git", "-C", m2, "config", "user.name", "t"], check=True)
        Store(m2).put("机二的条")
        sync(m2)
        # 机一再同步应拉到机二的条
        sync(self.data)
        s1 = Store(self.data)
        self.assertEqual(s1.count(), 2)

    def test_sync_rejects_non_git(self):
        from memlite.sync import sync, SyncError

        empty = os.path.join(self.dir, "empty")
        os.makedirs(empty)
        with self.assertRaises(SyncError):
            sync(empty)

    def test_credentials_not_staged(self):
        """凭据文件不进 git（.gitignore 白名单外的东西 add 不着）。"""
        from memlite.sync import sync

        s = Store(self.data)
        s.put("x")
        with open(os.path.join(self.data, "session.json"), "w") as f:
            f.write('{"token": "sekrit"}')
        with open(os.path.join(self.data, ".gitignore"), "w") as f:
            f.write("session*\n*.token\n")
        out = sync(self.data)
        clone = os.path.join(self.dir, "clone2")
        subprocess.run(["git", "clone", self.remote, clone], check=True, capture_output=True)
        self.assertFalse(os.path.exists(os.path.join(clone, "session.json")))
        self.assertTrue(out["verified"])


class InjectTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="memlite-inj-")
        self.home = os.path.join(self.dir, "home")
        os.makedirs(self.home)
        self._old_home = os.environ.get("HOME")
        os.environ["HOME"] = self.home

    def tearDown(self):
        os.environ["HOME"] = self._old_home
        shutil.rmtree(self.dir, ignore_errors=True)

    def test_inject_creates_block(self):
        from memlite import inject as injm

        out = injm.inject("generic", self.dir)
        self.assertTrue(out["changed"])
        path = os.path.join(self.home, "AGENTS.md")
        text = open(path).read()
        self.assertIn(injm.BEGIN, text)
        self.assertIn(injm.END, text)
        self.assertIn("memlite recall", text)

    def test_inject_idempotent(self):
        from memlite import inject as injm

        injm.inject("generic", self.dir)
        out2 = injm.inject("generic", self.dir)
        self.assertFalse(out2["changed"])

    def test_conflict_detected(self):
        from memlite import inject as injm

        path = os.path.join(self.home, "AGENTS.md")
        with open(path, "w") as f:
            f.write("# respire 记忆规则\nrsrs recall ...\n")
        out = injm.inject("generic", self.dir)
        self.assertEqual(out["state"], "conflict")
        self.assertFalse(out["changed"])
        # force 可注
        out2 = injm.inject("generic", self.dir, force=True)
        self.assertTrue(out2["changed"])

    def test_remove_block(self):
        from memlite import inject as injm

        path = os.path.join(self.home, "AGENTS.md")
        with open(path, "w") as f:
            f.write("# 用户原有内容\n")
        injm.inject("generic", self.dir)
        injm.remove("generic")
        text = open(path).read()
        self.assertNotIn(injm.BEGIN, text)
        self.assertIn("# 用户原有内容", text)

    def test_probe_no_write(self):
        from memlite import inject as injm

        injm.probe()
        self.assertFalse(os.path.exists(os.path.join(self.home, "AGENTS.md")))

    def test_workbuddy_lite(self):
        from memlite import inject as injm

        os.makedirs(os.path.join(self.home, ".workbuddy"), exist_ok=True)
        out = injm.inject("workbuddy", self.dir)
        text = open(out["path"]).read()
        self.assertIn(injm.BEGIN, text)
        self.assertLess(len(text), 1500)  # 4000 字符上限内


class CliTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="memlite-cli-")
        self._old_home = os.environ.get("HOME")
        os.environ["HOME"] = self.dir  # 隔离配置文件

    def tearDown(self):
        os.environ["HOME"] = self._old_home
        shutil.rmtree(self.dir, ignore_errors=True)

    def test_cli_end_to_end(self):
        from memlite.cli import main

        data = os.path.join(self.dir, "data")
        self.assertEqual(main(["--data-dir", data, "init"]), 0)
        self.assertEqual(main(["--data-dir", data, "remember", "memlite 用 JSONL 存储",
                               "--title", "存储设计", "--tags", "架构"]), 0)
        self.assertEqual(main(["--data-dir", data, "recall", "JSONL 存储"]), 0)
        self.assertEqual(main(["--data-dir", data, "list"]), 0)
        self.assertEqual(main(["--data-dir", data, "status"]), 0)


if __name__ == "__main__":
    unittest.main()


class TitleTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="memlite-title-")
        self._old_home = os.environ.get("HOME")
        os.environ["HOME"] = self.dir

    def tearDown(self):
        os.environ["HOME"] = self._old_home
        shutil.rmtree(self.dir, ignore_errors=True)

    def test_default_title_first_sentence(self):
        from memlite.cli import main
        from memlite.store import Store

        data = os.path.join(self.dir, "data")
        main(["--data-dir", data, "remember", "nginx 反代 504 超时。调大 proxy_read_timeout 解决。"])
        s = Store(data)
        rec = s.all()[0]
        self.assertEqual(rec["title"], "nginx 反代 504 超时")

    def test_default_title_truncated_40(self):
        from memlite.cli import main
        from memlite.store import Store

        data = os.path.join(self.dir, "data")
        main(["--data-dir", data, "remember", "很长的正文" * 30])
        s = Store(data)
        self.assertLessEqual(len(s.all()[0]["title"]), 40)

    def test_explicit_title_kept(self):
        from memlite.cli import main
        from memlite.store import Store

        data = os.path.join(self.dir, "data")
        main(["--data-dir", data, "remember", "正文内容", "--title", "自定义事实标题"])
        self.assertEqual(Store(data).all()[0]["title"], "自定义事实标题")


class EmbedTest(unittest.TestCase):
    """嵌入档：mock 客户端，不真打 API。"""

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="memlite-emb-")
        self._old_home = os.environ.get("HOME")
        os.environ["HOME"] = self.dir
        import memlite.embed as embedm
        self._real_get_client = embedm.get_client

    def tearDown(self):
        import memlite.embed as embedm
        embedm.get_client = self._real_get_client
        os.environ["HOME"] = self._old_home
        for k in ("MEMLITE_CONFIG",):
            os.environ.pop(k, None)
        shutil.rmtree(self.dir, ignore_errors=True)

    def _fake_client(self, mapping):
        """按关键词给向量的假客户端：同关键词向量相同→余弦 1。"""
        class Fake:
            def embed(self, text):
                from memlite.embed import EmbeddingError
                if "FAIL" in text:
                    raise EmbeddingError("模拟 API 挂了")
                base = [0.0] * 8
                for kw, idx in mapping.items():
                    if kw in text:
                        base[idx] = 1.0
                return base
        return Fake()

    def test_semantic_search_hits(self):
        from memlite.search import semantic_search
        recs = [
            {"id": "1", "ts": 1, "title": "", "content": "rust 编译", "tags": [], "project": "", "emb": [1.0, 0.0]},
            {"id": "2", "ts": 2, "title": "", "content": "菜谱", "tags": [], "project": "", "emb": [0.0, 1.0]},
        ]
        hits = semantic_search(recs, [1.0, 0.0], limit=2)
        self.assertEqual(hits[0][1]["id"], "1")

    def test_no_emb_records_skipped(self):
        from memlite.search import semantic_search
        recs = [{"id": "1", "ts": 1, "content": "无向量", "tags": [], "project": ""}]
        self.assertEqual(semantic_search(recs, [1.0], limit=5), [])

    def test_remember_embeds_when_configured(self):
        import memlite.embed as embedm
        from memlite.store import Store

        data = os.path.join(self.dir, "data")
        embedm.get_client = lambda: self._fake_client({"nginx": 0})
        os.environ["MEMLITE_CONFIG"] = os.path.join(self.dir, "nonexistent.json")
        from memlite.cli import main
        main(["--data-dir", data, "remember", "nginx 504 调 timeout"])
        rec = Store(data).all()[0]
        self.assertEqual(rec["emb"][0], 1.0)

    def test_recall_falls_back_on_api_error(self):
        import memlite.embed as embedm
        from memlite.store import Store

        data = os.path.join(self.dir, "data")
        s = Store(data)
        s.put("nginx 504 调 timeout 解决", title="nginx 超时")
        calls = {"n": 0}

        class Flaky:
            def embed(self, text):
                calls["n"] += 1
                from memlite.embed import EmbeddingError
                raise EmbeddingError("挂了")
        embedm.get_client = lambda: Flaky()
        os.environ["MEMLITE_CONFIG"] = os.path.join(self.dir, "nonexistent.json")
        from memlite.cli import main
        import io, contextlib
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = main(["--data-dir", data, "recall", "nginx 超时"])
        self.assertEqual(rc, 0)
        self.assertIn("nginx", buf.getvalue())  # BM25 兜底仍命中

    def test_recall_prefers_semantic(self):
        import memlite.embed as embedm
        from memlite.store import Store

        data = os.path.join(self.dir, "data")
        s = Store(data)
        s.put("容器编排 Pod 卡在 Pending", title="K8s Pod Pending", emb=[0.0, 1.0])
        s.put("菜谱", title="", emb=[1.0, 0.0])
        embedm.get_client = lambda: self._fake_client({"容器": 1})
        os.environ["MEMLITE_CONFIG"] = os.path.join(self.dir, "nonexistent.json")
        from memlite.cli import main
        import io, contextlib
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            main(["--data-dir", data, "recall", "容器编排"])
        self.assertIn("Pod Pending", buf.getvalue())

    def test_recall_bm25_flag_forces_lexical(self):
        from memlite.store import Store

        data = os.path.join(self.dir, "data")
        s = Store(data)
        s.put("rust 编译缓存", title="rust 编译", emb=[1.0, 0.0])
        from memlite.cli import main
        import io, contextlib
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            main(["--data-dir", data, "recall", "rust", "--bm25"])
        self.assertIn("rust", buf.getvalue())

    def test_reembed_backfills(self):
        import memlite.embed as embedm
        from memlite.store import Store

        data = os.path.join(self.dir, "data")
        s = Store(data)
        rid = s.put("旧条目没向量", title="旧条")
        embedm.get_client = lambda: self._fake_client({"旧条目": 2})
        os.environ["MEMLITE_CONFIG"] = os.path.join(self.dir, "nonexistent.json")
        from memlite.cli import main
        main(["--data-dir", data, "reembed"])
        s2 = Store(data)
        self.assertEqual(s2.get(rid)["emb"][2], 1.0)

    def test_reembed_requires_config(self):
        from memlite.store import Store
        data = os.path.join(self.dir, "data")
        Store(data).put("x", title="t")
        os.environ["MEMLITE_CONFIG"] = os.path.join(self.dir, "nonexistent.json")
        from memlite.cli import main
        self.assertEqual(main(["--data-dir", data, "reembed"]), 2)

    def test_status_reports_embedding(self):
        from memlite.store import Store
        data = os.path.join(self.dir, "data")
        Store(data).put("x", title="t")
        os.environ["MEMLITE_CONFIG"] = os.path.join(self.dir, "nonexistent.json")
        os.environ["MEMLITE_API_BASE"] = "https://api.example.com/v1"
        os.environ["MEMLITE_API_KEY"] = "dummy"
        os.environ["MEMLITE_EMBED_MODEL"] = "test-model"
        try:
            from memlite.cli import main
            import io, contextlib
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                main(["--data-dir", data, "status", "--json"])
            info = json.loads(buf.getvalue())
            self.assertTrue(info["embedding"]["configured"])
            self.assertNotIn("dummy", buf.getvalue())  # key 不回显
        finally:
            for k in ("MEMLITE_API_BASE", "MEMLITE_API_KEY", "MEMLITE_EMBED_MODEL"):
                os.environ.pop(k, None)

    def test_cosine_dimension_mismatch(self):
        from memlite.search import cosine
        self.assertEqual(cosine([1.0], [1.0, 2.0]), 0.0)
        self.assertEqual(cosine([], [1.0]), 0.0)
