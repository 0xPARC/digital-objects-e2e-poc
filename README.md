# digital-objects-e2e-poc

## Testing

- Run all tests: `cargo test --release -- --nocapture`
- Test specific package: `cargo test --release -p NAME_OF_PACKAGE -- --nocapture`

To run the integration flow, while also preparing the configuration and running the Synchronizer and the App, use the `full-flow.sh` script: `./full-flow.sh`
