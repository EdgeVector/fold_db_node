fn main() {
    let json = fold_db_node::server::openapi::build_openapi();
    println!("{}", json);
}
