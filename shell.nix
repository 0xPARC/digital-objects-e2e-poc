{ pkgs ? import <nixpkgs> { } }:

let
  dlopenLibraries = with pkgs; [
    libxkbcommon
    libxkbcommon.dev

    wayland
    wayland.dev
    wayland-protocols
    emacs
    mesa
    mesa-gl-headers
    egl-wayland
    libGL
  ];
in pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    rustup
    mesa
    mesa-gl-headers
  ];

  env.RUSTFLAGS = "-C link-arg=-Wl,-rpath,${pkgs.lib.makeLibraryPath dlopenLibraries}";
}
