import { readFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { isRangeSchema, getRangeKey, type Schema } from './src/utils/rangeSchemaHelpers';

const __dirname = dirname(fileURLToPath(import.meta.url));

interface SchemaField {
  field_type: string;
}

interface UserScoresSchemaData extends Schema {
  fields?: Record<string, SchemaField>;
}

const userScoresPath = join(__dirname, '../../../tests/schemas_for_testing/UserScores.json');
const userScoresSchema = JSON.parse(readFileSync(userScoresPath, 'utf8')) as UserScoresSchemaData;

console.log('UserScores Schema Analysis:');
console.log('=========================');
console.log('Schema name:', userScoresSchema.name);
console.log('Has schema_type:', !!userScoresSchema.schema_type);
console.log('Has Range in schema_type:', userScoresSchema.schema_type === 'Range');
console.log('Range key value:', getRangeKey(userScoresSchema));
console.log('Fields count:', Object.keys(userScoresSchema.fields || {}).length);

console.log('\nField types:');
Object.entries(userScoresSchema.fields ?? {}).forEach(([name, field]) => {
  console.log(`  ${name}: ${field.field_type}`);
});

console.log('\nAll fields are Range type:', Object.entries(userScoresSchema.fields ?? {}).every(([, field]) => field.field_type === 'Range'));
console.log('isRangeSchema result:', isRangeSchema(userScoresSchema));

// Test with minimal structure (schema_type is now a string, key holds the range_field)
const minimalTest: Schema = {
  name: 'UserScores',
  schema_type: 'Range',
  key: { range_field: 'user_id' },
  fields: {
    user_id: { field_type: 'Range' },
    game_scores: { field_type: 'Range' }
  }
};

console.log('\nMinimal test result:', isRangeSchema(minimalTest));
