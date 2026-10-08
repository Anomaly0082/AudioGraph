// Test-only native plugin. Reuse the public-SDK gain example's exact descriptor
// and implementation; interpose lifecycle/status callbacks for host fault tests.
#define ag_plugin_get_api fixture_base_get_api
#include "../examples/plugins/realtime/src/plugin.cpp"
#undef ag_plugin_get_api
#include <limits>
namespace {
uint32_t mode{}, creates{}, destroys{};
ag_status AG_CALL fixture_create(const ag_create_info* i,const ag_error_sink* e,ag_instance** out) {
    if(mode==5) { *out=nullptr; return AG_INTERNAL_ERROR; }
    if(mode==6) { *out=reinterpret_cast<ag_instance*>(uintptr_t{1}); return AG_INTERNAL_ERROR; }
    const auto result=create(i,e,out); if(result==AG_OK) ++creates; return result;
}
ag_status AG_CALL fixture_prepare(ag_instance* i,const ag_realtime_prepare_info* p,const ag_error_sink* e) {
    return mode==1 ? AG_INTERNAL_ERROR : prepare(i,p,e);
}
ag_realtime_status AG_CALL fixture_process(ag_instance* i,const ag_realtime_block* b) {
    if(mode==2) { b->samples[0]=0.75f; return AG_RT_NODE_FAILED; }
    if(mode==3) { b->samples[0]=0.75f; return 999; }
    if(mode==4) { b->samples[0]=std::numeric_limits<float>::infinity(); return AG_RT_OK; }
    return process(i,b);
}
void AG_CALL fixture_destroy(ag_instance* i) { ++destroys; destroy(i); }
ag_status AG_CALL fixture_get_node(ag_string t,ag_string c,uint32_t v,uint32_t n,void* out) {
    const auto result=get_node(t,c,v,n,out);
    if(result==AG_OK) {
        auto& a=*static_cast<ag_realtime_audio_api*>(out);
        a.create=fixture_create; a.prepare=fixture_prepare; a.process=fixture_process; a.destroy=fixture_destroy;
    }
    return result;
}
}
extern "C" AG_EXPORT void AG_CALL ag_rt_test_control(uint32_t value) { mode=value; }
extern "C" AG_EXPORT uint32_t AG_CALL ag_rt_test_creates() { return creates; }
extern "C" AG_EXPORT uint32_t AG_CALL ag_rt_test_destroys() { return destroys; }
extern "C" AG_EXPORT ag_status AG_CALL ag_plugin_get_api(uint32_t a,uint32_t b,uint32_t n,ag_plugin_api* out) {
    const auto result=fixture_base_get_api(a,b,n,out);
    if(result==AG_OK) out->get_node_api=fixture_get_node;
    return result;
}
