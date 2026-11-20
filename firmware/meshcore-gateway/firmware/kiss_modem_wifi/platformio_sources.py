from pathlib import Path

Import("env")

source_dir = Path(env.subst("$PROJECT_DIR")).parent / "firmware" / "kiss_modem_wifi"
object_dir = Path(env.subst("$BUILD_DIR")) / "maarse-gateway"

# Sources outside PlatformIO's project can otherwise share object files
# across environments, even when they use different display drivers.
env.VariantDir(str(object_dir), str(source_dir), duplicate=False)


def relocate_gateway_source(node):
    source = Path(node.srcnode().get_abspath())
    if source.parent == source_dir:
        return env.File(str(object_dir / source.name))
    return node


env.AddBuildMiddleware(relocate_gateway_source)
