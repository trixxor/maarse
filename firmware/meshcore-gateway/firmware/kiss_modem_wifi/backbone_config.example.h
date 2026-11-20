#pragma once

#define BACKBONE_HOST "backbone.example.invalid"
#define BACKBONE_PORT 5056
#define BACKBONE_GATEWAY_TOKEN "replace-with-a-unique-gateway-token"
static const char BACKBONE_CA_PEM[] = R"PEM(-----BEGIN CERTIFICATE-----
Replace with the PEM certificate for the issuing CA.
-----END CERTIFICATE-----
)PEM";
