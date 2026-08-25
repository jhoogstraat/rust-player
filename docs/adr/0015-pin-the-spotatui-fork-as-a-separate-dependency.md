# Pin the Spotatui fork as a separate dependency

The private Spotatui fork remains a separate repository and production builds consume its private Git URL at an exact revision. Coordinated development may use a documented local path override, but this repository will neither vendor nor submodule the fork; deliberate pin updates preserve reproducibility without coupling the two repositories' histories.
