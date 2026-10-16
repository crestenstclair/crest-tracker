#include "plugin_adapter.h"
namespace mda_Dynamics {
#include "../../vendor/audio/mda/plugins/mdaDynamics.cpp"

// Upstream leaves its three envelope histories uninitialized and inherits a
// no-op suspend. Own initialization/reset at the adapter boundary; retain the
// original compressor/gate algorithm and parameter calculations.
class CrestDynamics final : public mdaDynamics {
public:
    CrestDynamics() : mdaDynamics(reinterpret_cast<void*>(1)) { suspend(); }
    void suspend() override { env = env2 = genv = 0.f; }
};
}
CrestProcessor* make_mda_Dynamics(float rate, size_t frames) {
    return new CrestPlugin(new mda_Dynamics::CrestDynamics, rate, frames, false);
}
