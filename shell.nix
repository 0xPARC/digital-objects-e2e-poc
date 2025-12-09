{ pkgs ? import <nixpkgs> { } }:

let
  dlopenLibraries = with pkgs; [
    libxkbcommon
    # libxkbcommon.dev

    #vulkan-loader
    wayland
    # wayland.dev
    wayland-protocols
    sway
    mesa
    mesa-gl-headers
    egl-wayland
    libGL
  ];
in pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    rustup
    gcc
    pkg-config
  ];

  env.RUSTFLAGS = "-C linker=clang -C link-arg=-Wl,-rpath,${pkgs.lib.makeLibraryPath dlopenLibraries}";
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath dlopenLibraries;
}
