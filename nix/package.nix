{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "qssh";
  version = "0.1.0";

  src = lib.cleanSource ./..;

  cargoLock.lockFile = ../Cargo.lock;

  meta = with lib; {
    description = "QUIC-based SSH proxy with connection migration support";
    homepage = "https://github.com/rivers/qssh";
    license = licenses.mit;
    mainProgram = "qssh";
  };
}
