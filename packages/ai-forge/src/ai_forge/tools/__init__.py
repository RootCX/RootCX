"""Tools package — import all modules to populate the registry."""

# Import tool modules so their @register_tool decorators execute.
from ai_forge.tools import apps, build, components, file, integrations, web  # noqa: F401
