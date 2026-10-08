#include <audiograph/plugin.h>
_Static_assert(sizeof(ag_realtime_audio_api)==40,"C realtime table");
_Static_assert(sizeof(ag_realtime_prepare_info)==24,"C prepare");
_Static_assert(sizeof(ag_realtime_block)==32,"C block");
_Static_assert(offsetof(ag_realtime_audio_api,process)==24,"C process offset");
int ag_realtime_c11_smoke(const ag_realtime_audio_api* api) {
    ag_create_info info={0}; info.struct_size=sizeof(info);
    ag_instance* instance=NULL;
    if(api->create(&info,NULL,&instance)!=AG_OK || !instance) return 1;
    ag_realtime_prepare_info prepare={sizeof(prepare),0,48000,1,4,0};
    float samples[4]={0.25f,-0.5f,0.75f,0};
    ag_realtime_block block={sizeof(block),0,4,4,samples};
    int result=0;
    if(api->prepare(instance,&prepare,NULL)!=AG_OK || api->process(instance,&block)!=AG_RT_OK || samples[0]!=0.25f || samples[1]!=-0.5f || samples[2]!=0.75f || samples[3]!=0) result=2;
    api->destroy(instance); return result;
}
