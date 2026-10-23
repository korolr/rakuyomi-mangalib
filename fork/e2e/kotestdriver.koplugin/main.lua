local WidgetContainer = require("ui/widget/container/widgetcontainer")
local UIManager = require("ui/uimanager")
local Event = require("ui/event")
local logger = require("logger")
local D = WidgetContainer:extend{ name = "kotestdriver", is_doc_only = false }
function D:init()
  local ev = os.getenv("KO_TEST_EVENT")
  if ev and ev ~= "" and not _G.__kotest_fired then
    _G.__kotest_fired = true
    local i = 0
    for name in ev:gmatch("[^,]+") do
      i = i + 1
      UIManager:scheduleIn(4 * i, function()
        logger.warn("KOTEST firing", name)
        UIManager:broadcastEvent(Event:new(name))
      end)
    end
  end
end
return D
