// sfizz is built in Release mode by build_support/sfizz.rs. Its C++ parser
// types must use the same NDEBUG setting here: LeakDetector adds members in
// debug builds, making mixed definitions unsafe across the library boundary.
#ifndef NDEBUG
#define NDEBUG
#endif
#include "sfizz/parser/Parser.h"
#include "sfizz/parser/ParserListener.h"
#include "json.hpp"
#include <filesystem>
#include <fstream>
#include <set>
#include <map>
#include <sstream>
#include <cstring>
#include <cstdlib>

namespace {
std::string base64(const std::string& bytes){
    static const char alphabet[]="ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string output;output.reserve((bytes.size()+2)/3*4);
    for(size_t i=0;i<bytes.size();i+=3){
        const unsigned a=static_cast<unsigned char>(bytes[i]),b=i+1<bytes.size()?static_cast<unsigned char>(bytes[i+1]):0,c=i+2<bytes.size()?static_cast<unsigned char>(bytes[i+2]):0;
        output+=alphabet[a>>2];output+=alphabet[((a&3)<<4)|(b>>4)];output+=i+1<bytes.size()?alphabet[((b&15)<<2)|(c>>6)]:'=';output+=i+2<bytes.size()?alphabet[c&63]:'=';
    }return output;
}
struct Bundle:sfz::ParserListener {
    std::filesystem::path root;
    size_t budget, source_bytes=0;
    std::set<std::string> files;
    Bundle(std::filesystem::path folder,size_t limit):root(std::move(folder)),budget(limit) {}
    bool onParseFile(const std::string& path) override {
        const auto full=std::filesystem::canonical(path),relative=full.lexically_relative(root);
        if(relative.empty()||*relative.begin()=="..")return false;
        if(files.insert(full.string()).second) {
            const auto size=std::filesystem::file_size(full);
            if(size>budget-source_bytes)return false;
            source_bytes+=size;
        }
        return true;
    }
    std::vector<std::pair<std::string,std::vector<sfz::Opcode>>> blocks;
    void onParseFullBlock(const std::string& header,const std::vector<sfz::Opcode>& opcodes) override {blocks.emplace_back(header,opcodes);}
};
std::string bundle(const char* path,size_t budget){
    const auto file=std::filesystem::canonical(path),root=file.parent_path();
    sfz::Parser parser;Bundle listener(root,budget);parser.setListener(&listener);parser.setRecursiveIncludeGuardEnabled(true);parser.parseFile(path);
    if(parser.getErrorCount())throw std::invalid_argument("invalid SFZ");
    for(const auto& included:parser.getIncludedFiles()){
        const auto rel=std::filesystem::canonical(included).lexically_relative(root);
        if(rel.empty()||*rel.begin()=="..")throw std::invalid_argument("SFZ include escapes source folder");
    }
    std::set<std::string> embedded;
    for(const auto& block:listener.blocks)if(block.first=="sample")for(const auto& op:block.second)if(op.name=="name")embedded.insert(op.value);
    std::string default_path,head,body;std::map<std::string,std::string> aliases;size_t total=listener.source_bytes;
    for(auto& block:listener.blocks){
        body+="<"+block.first+">\n";
        for(auto& op:block.second){
            if(block.first=="control"&&op.name=="default_path"){default_path=op.value;std::replace(default_path.begin(),default_path.end(),'\\','/');continue;}
            if(op.name=="sample"&&!op.value.empty()&&op.value[0]!='*'&&!embedded.count(op.value)){
                auto name=op.value;std::replace(name.begin(),name.end(),'\\','/');
                const auto sample=std::filesystem::canonical(root/default_path/name),rel=sample.lexically_relative(root);
                if(rel.empty()||*rel.begin()=="..")throw std::invalid_argument("SFZ sample escapes source folder");
                auto found=aliases.find(sample.string());
                if(found==aliases.end()){
                    const auto size=std::filesystem::file_size(sample);if(size>budget-total)throw std::length_error("SFZ sample budget exceeded");total+=size;
                    std::ifstream input(sample,std::ios::binary);std::string bytes((std::istreambuf_iterator<char>(input)),{});
                    if(bytes.size()!=size)throw std::invalid_argument("SFZ sample read failed");
                    size_t number=aliases.size();
                    std::string alias;
                    do { alias="crest_embedded_"+std::to_string(number++)+sample.extension().string(); } while(embedded.count(alias));
                    embedded.insert(alias);
                    head+="<sample> name="+alias+" base64data="+base64(bytes)+"\n";
                    found=aliases.emplace(sample.string(),alias).first;
                }
                op.value=found->second;
            }
            body+=op.name+"="+op.value+"\n";
            if(head.size()+body.size()>budget*2)throw std::length_error("SFZ text budget exceeded");
        }
    }
    return head+body;
}
}
extern "C" char* crest_sfz_bundle(const char* path,size_t budget) noexcept {
    try {const auto text=bundle(path,budget);auto* result=static_cast<char*>(std::malloc(text.size()+1));if(result)std::memcpy(result,text.c_str(),text.size()+1);return result;}catch(...){return nullptr;}
}
extern "C" void crest_sfz_bundle_free(char* data) noexcept {std::free(data);}
