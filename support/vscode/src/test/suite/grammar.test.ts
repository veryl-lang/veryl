import * as assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { loadWASM, OnigScanner, OnigString } from 'vscode-oniguruma';
import { IGrammar, INITIAL, parseRawGrammar, Registry } from 'vscode-textmate';

suite('Veryl TextMate grammar', () => {
    let grammar: IGrammar;

    suiteSetup(async () => {
        const wasm = await readFile(require.resolve('vscode-oniguruma/release/onig.wasm'));
        await loadWASM(wasm);
        const grammarPath = resolve(__dirname, '../../../syntaxes/veryl.tmLanguage.json');
        const registry = new Registry({
            onigLib: Promise.resolve({
                createOnigScanner: patterns => new OnigScanner(patterns),
                createOnigString: text => new OnigString(text),
            }),
            loadGrammar: async scope =>
                scope === 'source.veryl'
                    ? parseRawGrammar(await readFile(grammarPath, 'utf8'), grammarPath)
                    : null,
        });
        const loaded = await registry.loadGrammar('source.veryl');
        assert.ok(loaded);
        grammar = loaded;
    });

    function scopedTokens(source: string, prefix: string): [string, string][] {
        const matches: [string, string][] = [];
        let ruleStack = INITIAL;
        for (const line of source.split('\n')) {
            const result = grammar.tokenizeLine(line, ruleStack);
            ruleStack = result.ruleStack;
            for (const token of result.tokens) {
                const scope = token.scopes[token.scopes.length - 1];
                if (scope?.startsWith(prefix)) {
                    matches.push([line.slice(token.startIndex, token.endIndex), scope]);
                }
            }
        }
        return matches;
    }

    test('scopes complete numeric literals and operators', () => {
        const line = "8'hff 16'shff_ff 'b01xz '0 1_024 1.5 1.25e-2 data_42 var$10";
        assert.deepEqual(scopedTokens(line, 'constant.numeric'), [
            ["8'hff", 'constant.numeric.integer.veryl'],
            ["16'shff_ff", 'constant.numeric.integer.veryl'],
            ["'b01xz", 'constant.numeric.integer.veryl'],
            ["'0", 'constant.numeric.integer.veryl'],
            ['1_024', 'constant.numeric.integer.veryl'],
            ['1.5', 'constant.numeric.float.veryl'],
            ['1.25e-2', 'constant.numeric.float.veryl'],
        ]);
        assert.deepEqual(
            scopedTokens('connect left <> right; value <<<= 2; Pkg::State::Idle', 'keyword.operator')
                .map(([text]) => text),
            ['<>', '<<<='],
        );
        assert.deepEqual(
            scopedTokens('Pkg::State::Idle', 'punctuation.separator.namespace').map(([text]) => text),
            ['::', '::'],
        );
    });

    test('scopes nested generic delimiters and import wildcards', () => {
        const source = [
            'let width: u32 = Types::<',
            '    Inner::<8>>::WIDTH;',
            'import Pkg:: /* wildcard */',
            '    *;',
            'let shifted: u32 = width >> 1;',
            'let doubled: u32 = width * 2;',
        ].join('\n');
        assert.deepEqual(
            scopedTokens(source, 'punctuation.definition.generic').map(([text]) => text),
            ['<', '<', '>', '>'],
        );
        assert.deepEqual(scopedTokens(source, 'punctuation.definition.wildcard'), [
            ['*', 'punctuation.definition.wildcard.veryl'],
        ]);
        assert.deepEqual(
            scopedTokens(source, 'keyword.operator').map(([text]) => text),
            ['=', '=', '>>', '=', '*'],
        );
    });

    test('scopes declared names across comments and line breaks', () => {
        const text = [
            'module Top {}',
            'interface Bus {}',
            'package Pkg {}',
            'function // declaration',
            '    /* name */ r#module() {}',
            'struct Word {}',
            'enum State {}',
        ].join('\n');
        assert.deepEqual(scopedTokens(text, 'entity.name'), [
            ['Top', 'entity.name.type.module.veryl'],
            ['Bus', 'entity.name.type.interface.veryl'],
            ['Pkg', 'entity.name.type.namespace.veryl'],
            ['r#module', 'entity.name.function.veryl'],
            ['Word', 'entity.name.type.struct.veryl'],
            ['State', 'entity.name.type.enum.veryl'],
        ]);
    });

    test('preserves comments, strings, raw identifiers and abstract interface ports', () => {
        const text = [
            '/* module Hidden {}',
            "    8'hff <> */",
            '"enum Hidden {} 123 <>";',
            '// function hidden() {}',
            'r#if module$0;',
            'bus: interface, value: input logic,',
        ].join('\n');
        assert.deepEqual(scopedTokens(text, 'entity.name'), []);
        assert.deepEqual(scopedTokens(text, 'constant.numeric'), []);
        assert.deepEqual(scopedTokens(text, 'keyword.operator'), []);
        assert.deepEqual(scopedTokens(text, 'variable.other.identifier').map(([word]) => word),
            ['r#if', 'module$0', 'bus', 'value']);
        assert.deepEqual(scopedTokens(text, 'storage.modifier').map(([word]) => word), ['input']);
    });

    test('scopes dollar-prefixed identifiers', () => {
        const text = 'assign a = $clog2(WIDTH); inst w: $std::fifo; var t: $sv::Word;';
        assert.deepEqual(
            scopedTokens(text, 'variable.other.identifier').map(([word]) => word),
            ['a', '$clog2', 'WIDTH', 'w', '$std', 'fifo', 't', '$sv', 'Word'],
        );
    });
});
