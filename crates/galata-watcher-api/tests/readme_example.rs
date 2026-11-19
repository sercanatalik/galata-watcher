//! The example configuration in the README is accepted as written.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use galata_watcher::files::Paths;
use galata_watcher_api::{App, router};
use tower::ServiceExt;

fn readme_example() -> String {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")).unwrap();
    let after = readme.split("<!-- example-config").nth(1).expect("the README marks its example");
    let block = after.split("```json").nth(1).unwrap();
    block.split("```").next().unwrap().to_string()
}

#[tokio::test]
async fn the_readme_example_is_accepted_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let req = Request::builder()
        .method("PUT")
        .uri("/config")
        .header(header::AUTHORIZATION, "Bearer t")
        .header(header::IF_MATCH, "\"0\"")
        .body(Body::from(readme_example()))
        .unwrap();
    let res = router(App::new(Paths::new(dir.path()), "t".into())).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}
