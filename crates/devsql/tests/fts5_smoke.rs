#[test]
fn fts5_available() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE VIRTUAL TABLE t USING fts5(x); INSERT INTO t VALUES ('hello world');").unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM t WHERE t MATCH 'hello'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}
