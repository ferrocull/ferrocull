fn main() -> ferrocull_ui::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ferrocull=debug".parse().expect("valid directive")),
        )
        .init();

    ferrocull_ui::run()
}
