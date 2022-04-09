#[cfg(target_os = "macos")]
use core_foundation::runloop::CFRunLoop;
use kiro_midi::{self as midi, drivers::DriverSpec};

fn main() {
  let mut driver = midi::drivers::create("test").unwrap();

  let input_config1 = midi::InputConfig::new("all").with_source(
    midi::SourceMatch::regex(".*").unwrap(),
    midi::Filter::default(),
  );

  let input_config2 = midi::InputConfig::new("novation").with_source(
    midi::SourceMatch::regex("novation.*").unwrap(),
    midi::Filter::default(),
  );


  driver
    .create_input(input_config1, |event| println!("all >> {:?}", event))
    .unwrap();

  driver
    .create_input(input_config2, |event| println!("novation >> {:?}", event))
    .unwrap();

  println!("Inputs created");

  print_endpoints(&driver);

  loop {
    loop {
      let mut input_line = String::new();
      std::io::stdin()
        .read_line(&mut input_line)
        .expect("Failed to read line");

      print_endpoints(&driver);

      if let Some(mut input_config) = driver.get_input_config("jack") {
        // dbg!(&input_config);
        input_config.sources.add_source(
          midi::SourceMatch::regex(".*").unwrap(),
          midi::Filter::default(),
        );

        driver
          .set_input_sources(
            "jack",
            midi::SourceMatches::default().with_source(
              midi::SourceMatch::regex(".*").unwrap(),
              midi::Filter::default(),
            ),
          )
          .ok();
      }
    }
  }
}

fn print_endpoints(driver: &midi::drivers::Driver) {
  println!("===================================================================================");
  println!("Sources:");
  for mut source in driver.sources() {
    let input_names = (!source.connected_inputs.is_empty())
      .then(|| {
        source.connected_inputs.sort();
        format!(" ({})", source.connected_inputs.join(", "))
      })
      .unwrap_or_default();
    println!("  [{:08x}] {} {}", source.id, source.name, input_names);
  }
  println!("Destinations:");
  for destination in driver.destinations() {
    println!("  [{:08x}] {}", destination.id, destination.name);
  }
  println!("===================================================================================");
}
