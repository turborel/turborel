//! Spatial SQL through the shipped server, including reopen and opaque clients.

use turborel_client::connection::{self, Trust};
use turborel_test_support::Scratch;
use turborel_test_support::client::rows;

use crate::program::{config, running};

/// Verifies: [EXTENSION] D82; [DESIGN] D25, D45: the shipped server
/// registers GEOMETRY and its functions before opening files, so SQL-only
/// clients can create geometry data, query it, and read it after restart.
#[test]
fn spatial_sql_is_available_in_the_server_and_after_reopen() {
    let scratch = Scratch::new("server-geometry");
    let arguments = vec![
        "--dir".into(),
        scratch.path("db").display().to_string(),
        "--set".into(),
        "SERVER.PORT=0".into(),
        "--set".into(),
        "SERVER.WORKERS=2".into(),
        "--set".into(),
        "SERVER.TRUST_LOCAL=ON".into(),
        "--log".into(),
        scratch.path("log").display().to_string(),
    ];
    for first in [true, false] {
        let (addresses, stop, thread) = running(config(&arguments));
        let mut client = connection::connect(addresses[0].parse::<std::net::SocketAddr>().unwrap(), "G", &mut Trust).unwrap();
        client.query("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
        if first {
            client.query("CREATE SCHEMA AUTHORIZATION G CREATE TABLE SHAPES (G GEOMETRY(4326))").unwrap();
            client.query("INSERT INTO SHAPES VALUES (ST_POINT(4.9, 52.37, 4326))").unwrap();
            client.query("COMMIT").unwrap();
        }
        assert_eq!(
            rows(&client.query("SELECT ST_ASTEXT(G), ST_SRID(G), ST_INTERSECTS(G, ST_POINT(4.9, 52.37, 4326)) FROM SHAPES").unwrap()),
            ["POINT (4.9 52.37),4326,1"]
        );
        assert_eq!(rows(&client.query("SELECT G FROM SHAPES").unwrap()), ["POINT (4.9 52.37)"]);
        client.terminate().unwrap();
        drop(stop);
        thread.join().unwrap().unwrap();
    }
}
